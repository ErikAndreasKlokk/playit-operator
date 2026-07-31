use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Network protocol exposed by a tunnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    /// TCP only.
    #[default]
    Tcp,
    /// UDP only.
    Udp,
    /// Both TCP and UDP on the same port.
    Both,
}

/// `PlayitTunnel` declares a playit.gg tunnel that forwards public traffic to a
/// Kubernetes `Service`. The operator reconciles it against the playit.gg API
/// and writes the assigned public address back into [`PlayitTunnelStatus`].
///
/// playit is a layer-4 (TCP/UDP) port allocator, so each `PlayitTunnel` maps one
/// public address to one `Service` port — unlike an HTTP ingress there is no
/// host-header routing.
#[derive(CustomResource, Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[kube(
    group = "playit-operator.io",
    version = "v1alpha1",
    kind = "PlayitTunnel",
    namespaced,
    status = "PlayitTunnelStatus",
    shortname = "ptun",
    printcolumn = r#"{"name":"Protocol","type":"string","jsonPath":".spec.protocol"}"#,
    printcolumn = r#"{"name":"Service","type":"string","jsonPath":".spec.serviceName"}"#,
    printcolumn = r#"{"name":"Address","type":"string","jsonPath":".status.address"}"#,
    printcolumn = r#"{"name":"Phase","type":"string","jsonPath":".status.phase"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct PlayitTunnelSpec {
    /// Protocol to expose over the tunnel. Defaults to `tcp`.
    #[serde(default)]
    pub protocol: Protocol,

    /// Name of the target `Service`, in the same namespace as this resource.
    pub service_name: String,

    /// Port on the target `Service` to forward traffic to.
    pub port: u16,

    /// Number of consecutive ports to allocate, for services that need a port
    /// range. Defaults to 1.
    #[serde(default = "default_port_count")]
    pub port_count: u16,

    /// Optional region / endpoint preference (a playit region id). When unset,
    /// playit.gg picks the allocation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,

    /// Custom domain to attach to this tunnel (requires playit Premium).
    ///
    /// Reserved but **not yet implemented** — declared now so that turning on
    /// custom-domain support later is a non-breaking change. See the README
    /// roadmap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_domain: Option<String>,
}

fn default_port_count() -> u16 {
    1
}

/// Observed state of a [`PlayitTunnel`], written to `.status` by the operator.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlayitTunnelStatus {
    /// High-level lifecycle phase: `Pending`, `Ready`, or `Error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,

    /// Opaque tunnel id returned by playit.gg.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel_id: Option<String>,

    /// Public address (`host:port`) assigned to the tunnel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,

    /// Whether the requested custom domain has been attached and is serving.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_domain_ready: Option<bool>,

    /// `.metadata.generation` last successfully reconciled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,

    /// Human-readable detail about the current phase (progress or error text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}
