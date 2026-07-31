use std::sync::Arc;

use kube::Client;
use tracing::info;
use tracing_subscriber::{prelude::*, EnvFilter};

use playit_operator::controller;
use playit_operator::provider::{DryRunProvider, PlayitProvider, TunnelProvider};

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
            let key = std::env::var("PLAYIT_API_KEY").map_err(|_| {
                anyhow::anyhow!("PLAYIT_PROVIDER=playit requires PLAYIT_API_KEY to be set")
            })?;
            info!("using playit.gg API provider");
            Arc::new(PlayitProvider::new(key))
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
