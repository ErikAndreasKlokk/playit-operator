use async_trait::async_trait;

use super::{DesiredTunnel, ProvisionedTunnel, TunnelProvider};
use crate::error::{Error, Result};

/// Talks to the real playit.gg API (`https://api.playit.gg`).
///
/// # Wiring plan (tracked in the README roadmap)
///
/// The official `playit-api-client` crate (and `playit-api-java`) authenticate
/// with an account **`ApiKey`** via the `Authorization` header and expose
/// `tunnels/create`, `tunnels/list`, `tunnels/update` and `tunnels/delete`.
/// Reconciliation should:
///
/// 1. `tunnels/list` and find the tunnel tagged with `desired.key`.
/// 2. `tunnels/create` (or `tunnels/update`) to set protocol, port(s) and the
///    local address, so the running agent picks it up via `AgentRunDataV1`.
/// 3. Attach `desired.custom_domain` when present (requires playit Premium).
///
/// Until that is implemented, every method returns [`Error::NotImplemented`] so
/// the operator fails loudly rather than silently doing nothing.
pub struct PlayitProvider {
    #[allow(dead_code)]
    api_key: String,
    #[allow(dead_code)]
    base_url: String,
}

impl PlayitProvider {
    /// Construct a provider that authenticates with the given account API key.
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: "https://api.playit.gg".to_string(),
        }
    }
}

#[async_trait]
impl TunnelProvider for PlayitProvider {
    async fn ensure(&self, _desired: &DesiredTunnel) -> Result<ProvisionedTunnel> {
        Err(Error::NotImplemented("PlayitProvider::ensure"))
    }

    async fn delete(&self, _key: &str) -> Result<()> {
        Err(Error::NotImplemented("PlayitProvider::delete"))
    }
}
