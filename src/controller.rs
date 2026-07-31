//! The reconcile loop: watch `PlayitTunnel` resources and drive the configured
//! [`TunnelProvider`] to match, managing a finalizer so tunnels are torn down on
//! deletion.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use k8s_openapi::api::core::v1::Service;
use kube::api::{Patch, PatchParams};
use kube::runtime::controller::Action;
use kube::runtime::finalizer::{finalizer, Event as Finalizer};
use kube::runtime::watcher::Config;
use kube::runtime::Controller;
use kube::{Api, Client, Resource, ResourceExt};
use serde_json::json;
use tracing::{error, info, warn};

use crate::crd::{PlayitTunnel, PlayitTunnelStatus};
use crate::error::{Error, Result};
use crate::provider::{DesiredTunnel, TunnelProvider};

/// Finalizer added to every `PlayitTunnel` so deletion is intercepted.
pub const FINALIZER: &str = "playit-operator.io/finalizer";

/// Shared state handed to every reconcile call.
pub struct Context {
    pub client: Client,
    pub provider: Arc<dyn TunnelProvider>,
}

/// Start the controller and run until the process receives a shutdown signal.
pub async fn run(client: Client, provider: Arc<dyn TunnelProvider>) -> Result<()> {
    let tunnels: Api<PlayitTunnel> = Api::all(client.clone());
    let ctx = Arc::new(Context {
        client: client.clone(),
        provider,
    });

    // Fail fast with a helpful message if the CRD isn't installed yet.
    if let Err(e) = tunnels.list(&Default::default()).await {
        error!("Unable to list PlayitTunnels — is the CRD installed? ({e})");
        return Err(e.into());
    }

    info!("starting PlayitTunnel controller");
    Controller::new(tunnels, Config::default())
        .shutdown_on_signal()
        .run(reconcile, error_policy, ctx)
        .for_each(|res| async move {
            match res {
                Ok(o) => info!("reconciled {o:?}"),
                Err(e) => warn!("reconcile loop error: {e}"),
            }
        })
        .await;

    Ok(())
}

async fn reconcile(tunnel: Arc<PlayitTunnel>, ctx: Arc<Context>) -> Result<Action> {
    let ns = tunnel
        .namespace()
        .ok_or(Error::MissingField("metadata.namespace"))?;
    let name = tunnel.name_any();
    info!("reconciling PlayitTunnel {ns}/{name}");

    let api: Api<PlayitTunnel> = Api::namespaced(ctx.client.clone(), &ns);
    finalizer(&api, FINALIZER, tunnel, |event| async {
        match event {
            Finalizer::Apply(t) => apply(t, &ctx, &ns).await,
            Finalizer::Cleanup(t) => cleanup(t, &ctx).await,
        }
    })
    .await
    .map_err(|e| Error::Provider(e.to_string()))
}

/// Ensure the tunnel exists and reflect the result in `.status`.
async fn apply(tunnel: Arc<PlayitTunnel>, ctx: &Arc<Context>, ns: &str) -> Result<Action> {
    let name = tunnel.name_any();
    let key = format!("{ns}/{name}");
    let spec = &tunnel.spec;

    // Resolve the target Service so we don't allocate a tunnel to nothing.
    let services: Api<Service> = Api::namespaced(ctx.client.clone(), ns);
    let svc = services
        .get_opt(&spec.service_name)
        .await?
        .ok_or_else(|| Error::ServiceNotFound(spec.service_name.clone(), ns.to_string()))?;

    let cluster_ip = svc
        .spec
        .as_ref()
        .and_then(|s| s.cluster_ip.clone())
        .filter(|ip| ip != "None")
        .unwrap_or_else(|| format!("{}.{ns}.svc.cluster.local", spec.service_name));

    let desired = DesiredTunnel::from_spec(key.clone(), spec, cluster_ip);

    match ctx.provider.ensure(&desired).await {
        Ok(t) => {
            info!("{key}: tunnel ready at {}", t.address);
            let status = PlayitTunnelStatus {
                phase: Some("Ready".into()),
                tunnel_id: Some(t.tunnel_id),
                address: Some(t.address),
                custom_domain_ready: Some(t.custom_domain_ready),
                observed_generation: tunnel.meta().generation,
                message: Some("tunnel provisioned".into()),
            };
            patch_status(ctx, ns, &name, tunnel.status.as_ref(), status).await?;
            // Steady state: re-check periodically to detect and repair drift.
            Ok(Action::requeue(Duration::from_secs(300)))
        }
        Err(e) => {
            warn!("{key}: provider.ensure failed: {e}");
            let status = PlayitTunnelStatus {
                phase: Some("Error".into()),
                message: Some(e.to_string()),
                observed_generation: tunnel.meta().generation,
                ..Default::default()
            };
            patch_status(ctx, ns, &name, tunnel.status.as_ref(), status).await?;
            Ok(Action::requeue(Duration::from_secs(30)))
        }
    }
}

/// Tear the tunnel down when the resource is being deleted.
async fn cleanup(tunnel: Arc<PlayitTunnel>, ctx: &Arc<Context>) -> Result<Action> {
    let ns = tunnel.namespace().unwrap_or_default();
    let key = format!("{ns}/{}", tunnel.name_any());
    info!("cleaning up PlayitTunnel {key}");
    ctx.provider.delete(&key).await?;
    Ok(Action::await_change())
}

async fn patch_status(
    ctx: &Arc<Context>,
    ns: &str,
    name: &str,
    current: Option<&PlayitTunnelStatus>,
    status: PlayitTunnelStatus,
) -> Result<()> {
    // Skip the patch when nothing changed — a status write bumps the resource
    // version and re-triggers reconcile, so patching unconditionally spins.
    if current == Some(&status) {
        return Ok(());
    }
    let api: Api<PlayitTunnel> = Api::namespaced(ctx.client.clone(), ns);
    let patch = json!({
        "apiVersion": "playit-operator.io/v1alpha1",
        "kind": "PlayitTunnel",
        "status": status,
    });
    api.patch_status(name, &PatchParams::default(), &Patch::Merge(&patch))
        .await?;
    Ok(())
}

fn error_policy(_obj: Arc<PlayitTunnel>, err: &Error, _ctx: Arc<Context>) -> Action {
    warn!("reconcile error, requeuing: {err}");
    Action::requeue(Duration::from_secs(15))
}
