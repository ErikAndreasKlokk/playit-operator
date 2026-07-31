//! Abstraction over "something that can create/update/delete playit tunnels".
//!
//! The reconcile loop depends only on the [`TunnelProvider`] trait. Two
//! implementations are provided:
//!
//! * [`DryRunProvider`] — performs no external calls, logs the operations it
//!   *would* run and returns a deterministic synthetic address. Lets the whole
//!   control loop be developed and demoed without playit credentials.
//! * [`PlayitProvider`] — talks to the real `https://api.playit.gg`. Currently a
//!   stub; see its docs for the wiring plan.

use async_trait::async_trait;

use crate::crd::{PlayitTunnelSpec, Protocol};
use crate::error::Result;

/// The desired shape of a tunnel, distilled from a [`PlayitTunnelSpec`] plus the
/// resolved in-cluster target address.
#[derive(Debug, Clone)]
pub struct DesiredTunnel {
    /// Stable key (`namespace/name`) correlating a CR with a playit tunnel.
    pub key: String,
    pub protocol: Protocol,
    pub port: u16,
    pub port_count: u16,
    pub region: Option<String>,
    pub custom_domain: Option<String>,
    /// In-cluster target the agent forwards to, e.g. `10.0.0.5:8080`.
    pub local_target: String,
}

impl DesiredTunnel {
    /// Build a [`DesiredTunnel`] from a spec and the resolved cluster target.
    pub fn from_spec(key: String, spec: &PlayitTunnelSpec, local_target: String) -> Self {
        Self {
            key,
            protocol: spec.protocol,
            port: spec.port,
            port_count: spec.port_count,
            region: spec.region.clone(),
            custom_domain: spec.custom_domain.clone(),
            local_target,
        }
    }
}

/// The realized tunnel as reported by a provider.
#[derive(Debug, Clone)]
pub struct ProvisionedTunnel {
    pub tunnel_id: String,
    /// Public `host:port` assigned by playit.
    pub address: String,
    pub custom_domain_ready: bool,
}

/// Anything that can reconcile a desired tunnel against a backend.
#[async_trait]
pub trait TunnelProvider: Send + Sync {
    /// Ensure a tunnel matching `desired` exists, creating or updating as
    /// needed. Must be idempotent.
    async fn ensure(&self, desired: &DesiredTunnel) -> Result<ProvisionedTunnel>;

    /// Remove the tunnel associated with `key`. Must be idempotent.
    async fn delete(&self, key: &str) -> Result<()>;
}

mod dryrun;
mod playit;

pub use dryrun::DryRunProvider;
pub use playit::PlayitProvider;
