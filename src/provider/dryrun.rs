use async_trait::async_trait;
use tracing::info;

use super::{DesiredTunnel, ProvisionedTunnel, TunnelProvider};
use crate::error::Result;

/// A provider that performs no external calls. It logs the API operations it
/// *would* perform and returns a deterministic synthetic address, so the
/// reconcile loop can be developed and demoed without playit credentials.
///
/// This is the default provider unless `PLAYIT_PROVIDER=playit` is set.
#[derive(Debug, Default)]
pub struct DryRunProvider;

impl DryRunProvider {
    fn slug(key: &str) -> String {
        key.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .trim_matches('-')
            .to_string()
    }
}

#[async_trait]
impl TunnelProvider for DryRunProvider {
    async fn ensure(&self, desired: &DesiredTunnel) -> Result<ProvisionedTunnel> {
        let slug = Self::slug(&desired.key);
        let address = format!("{slug}.dryrun.playit.gg:{}", desired.port);
        info!(
            key = %desired.key,
            protocol = ?desired.protocol,
            port_count = desired.port_count,
            local_target = %desired.local_target,
            custom_domain = ?desired.custom_domain,
            "[dry-run] would ensure playit tunnel -> {address}"
        );
        Ok(ProvisionedTunnel {
            tunnel_id: format!("dryrun-{slug}"),
            address,
            custom_domain_ready: desired.custom_domain.is_some(),
        })
    }

    async fn delete(&self, key: &str) -> Result<()> {
        info!(key = %key, "[dry-run] would delete playit tunnel");
        Ok(())
    }
}
