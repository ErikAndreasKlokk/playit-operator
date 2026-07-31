use std::sync::Arc;

use kube::Client;
use tracing::{info, warn};
use tracing_subscriber::{prelude::*, EnvFilter};

use playit_operator::controller;
use playit_operator::provider::{DryRunProvider, PlayitCredential, PlayitProvider, TunnelProvider};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Select the backend. Default to dry-run so the operator is safe to run
    // without credentials; opt into the real API with PLAYIT_PROVIDER=playit.
    let provider: Arc<dyn TunnelProvider> = match std::env::var("PLAYIT_PROVIDER").as_deref() {
        Ok("playit") => {
            // The agent key is the credential now: a *self-managed* agent can
            // create/modify its own tunnels via the V1 API. Account API keys were
            // cancelled by playit (abuse), so PLAYIT_API_KEY is a dead fallback.
            let cred = if let Ok(k) = std::env::var("PLAYIT_AGENT_KEY") {
                info!(
                    "using playit.gg V1 API with an agent key (must be a self-managed agent to \
                     create/modify tunnels)"
                );
                PlayitCredential::AgentKey(k)
            } else if let Ok(k) = std::env::var("PLAYIT_API_KEY") {
                warn!(
                    "using PLAYIT_API_KEY, but playit cancelled account API keys — prefer a \
                     self-managed agent's PLAYIT_AGENT_KEY"
                );
                PlayitCredential::ApiKey(k)
            } else {
                return Err(anyhow::anyhow!(
                    "PLAYIT_PROVIDER=playit requires PLAYIT_AGENT_KEY (a self-managed agent's secret)"
                ));
            };
            Arc::new(PlayitProvider::new(cred))
        }
        _ => {
            info!("using dry-run provider (set PLAYIT_PROVIDER=playit to enable the real API)");
            Arc::new(DryRunProvider)
        }
    };

    let client = Client::try_default().await?;
    controller::run(client, provider).await?;
    Ok(())
}
