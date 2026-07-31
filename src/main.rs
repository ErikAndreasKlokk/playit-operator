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
            // Prefer a write-capable account API key; fall back to the agent key
            // (read-only — good for listing, but tunnel creation will be rejected
            // by playit until an API key is supplied).
            let cred = if let Ok(k) = std::env::var("PLAYIT_API_KEY") {
                info!("using playit.gg API provider with an account API key (write-capable)");
                PlayitCredential::ApiKey(k)
            } else if let Ok(k) = std::env::var("PLAYIT_AGENT_KEY") {
                warn!(
                    "using playit.gg API provider with an AGENT key — this is READ-ONLY; \
                     tunnel creation/updates will fail until PLAYIT_API_KEY is set"
                );
                PlayitCredential::AgentKey(k)
            } else {
                return Err(anyhow::anyhow!(
                    "PLAYIT_PROVIDER=playit requires PLAYIT_API_KEY (preferred, write-capable) \
                     or PLAYIT_AGENT_KEY (read-only)"
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
