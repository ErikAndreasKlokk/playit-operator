//! The real provider: talks to the playit.gg API (`https://api.playit.gg`) as a
//! **self-managed agent** using the **V1** tunnel API.
//!
//! Background: playit cancelled account API keys (abuse), so the account
//! `/tunnels/*` write endpoints are unusable. Instead, an agent claimed as
//! `self-managed` (`permissions.is_self_managed == true`) can create and manage
//! *its own* tunnels via the V1 API with just its agent key. This mirrors the
//! official `playit-minecraft-plugin`. See the repo `CLAUDE.md` for the claim
//! flow that provisions a self-managed key.
//!
//! The API is RPC-style — every call is `POST /<path>` with a JSON body and an
//! `Authorization: <Kind>-Key <secret>` header, returning an envelope of the
//! form `{"status":"success","data":…}` or `{"status":"error","data":{…}}`.

use async_trait::async_trait;
use reqwest::header::AUTHORIZATION;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use super::{DesiredTunnel, ProvisionedTunnel, TunnelProvider};
use crate::crd::Protocol;
use crate::error::{Error, Result};

const API_BASE: &str = "https://api.playit.gg";

/// A credential for the playit.gg API — becomes an `Authorization` header.
#[derive(Clone)]
pub enum PlayitCredential {
    /// Agent secret key. A **self-managed** agent can create its own tunnels via
    /// the V1 API; an assignable agent is read-only. This is the credential to
    /// use (as `PLAYIT_AGENT_KEY`).
    AgentKey(String),
    /// Account API key — playit **cancelled** these (abuse). Kept only so the
    /// header type exists; not a working write path.
    ApiKey(String),
}

impl PlayitCredential {
    fn header_value(&self) -> String {
        match self {
            PlayitCredential::AgentKey(v) => format!("Agent-Key {}", v.trim()),
            PlayitCredential::ApiKey(v) => format!("Api-Key {}", v.trim()),
        }
    }
}

/// Provider backed by the live playit.gg V1 API.
pub struct PlayitProvider {
    http: reqwest::Client,
    auth_header: String,
}

impl PlayitProvider {
    /// Construct a provider that authenticates with the given credential.
    pub fn new(credential: PlayitCredential) -> Self {
        Self {
            http: reqwest::Client::new(),
            auth_header: credential.header_value(),
        }
    }

    /// POST `body` to `path` and return the `data` field, translating the playit
    /// error envelope into an [`Error::Provider`].
    async fn call_raw<B: Serialize>(&self, path: &str, body: &B) -> Result<serde_json::Value> {
        let url = format!("{API_BASE}{path}");
        let resp = self
            .http
            .post(&url)
            .header(AUTHORIZATION, &self.auth_header)
            .json(body)
            .send()
            .await
            .map_err(|e| Error::Provider(format!("HTTP {path}: {e}")))?;
        let text = resp
            .text()
            .await
            .map_err(|e| Error::Provider(format!("reading {path} response: {e}")))?;
        let v: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| Error::Provider(format!("decoding {path} response: {e}; body={text}")))?;

        if v.get("status").and_then(|s| s.as_str()) == Some("success") {
            return Ok(v.get("data").cloned().unwrap_or(serde_json::Value::Null));
        }
        let kind = v
            .pointer("/data/type")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown");
        let msg = v
            .pointer("/data/message")
            .and_then(|s| s.as_str())
            .unwrap_or("");
        Err(Error::Provider(format!("playit {path}: {kind} — {msg}")))
    }

    /// Like [`Self::call_raw`] but deserializes `data` into `R`.
    async fn call<B: Serialize, R: DeserializeOwned>(&self, path: &str, body: &B) -> Result<R> {
        let data = self.call_raw(path, body).await?;
        serde_json::from_value(data)
            .map_err(|e| Error::Provider(format!("decoding {path} data: {e}")))
    }

    /// Fetch this agent's run data: its id, self-managed status, and its tunnels.
    async fn rundata(&self) -> Result<AgentRunDataV1> {
        self.call("/v1/agents/rundata", &Empty {}).await
    }

    async fn create_tunnel(&self, desired: &DesiredTunnel, agent_id: &str) -> Result<String> {
        // Default to the "global" network when no region is requested; override
        // per-tunnel with `spec.region`.
        let region = desired
            .region
            .clone()
            .unwrap_or_else(|| "global".to_string());
        let req = ReqTunnelsCreateV1 {
            name: tunnel_name(&desired.key),
            protocol: build_protocol(desired),
            origin: OriginCreate::Agent(AgentOrigin {
                agent_id: Some(agent_id.to_string()),
                config: local_config(&desired.local_ip, desired.local_port),
            }),
            endpoint: EndpointCreate::Region(UseAllocRegion { region, port: None }),
            enabled: true,
            firewall_id: None,
        };
        let obj: ObjectId = self.call("/v1/tunnels/create", &req).await?;
        Ok(obj.id)
    }

    async fn config_tunnel(&self, tunnel_id: &str, desired: &DesiredTunnel) -> Result<()> {
        let req = ReqTunnelsConfigV1 {
            tunnel_id: tunnel_id.to_string(),
            new_agent_id: None,
            new_config: Some(local_config(&desired.local_ip, desired.local_port)),
        };
        self.call_raw("/v1/tunnels/config", &req).await?;
        Ok(())
    }

    async fn delete_tunnel(&self, tunnel_id: &str) -> Result<()> {
        // The V1 API has no delete endpoint; `/tunnels/delete` works for a
        // self-managed agent's own tunnels (verified live).
        let req = ReqDelete {
            tunnel_id: tunnel_id.to_string(),
        };
        self.call_raw("/tunnels/delete", &req).await?;
        Ok(())
    }
}

#[async_trait]
impl TunnelProvider for PlayitProvider {
    async fn ensure(&self, desired: &DesiredTunnel) -> Result<ProvisionedTunnel> {
        if desired.local_ip.parse::<std::net::IpAddr>().is_err() {
            return Err(Error::Provider(format!(
                "resolved local_ip `{}` is not an IP address — playit forwards to a ClusterIP, \
                 so headless Services (clusterIP: None) aren't supported",
                desired.local_ip
            )));
        }

        let name = tunnel_name(&desired.key);
        let rd = self.rundata().await?;

        let (tunnel_id, address, custom_domain_ready) = match rd
            .tunnels
            .iter()
            .find(|t| t.name.as_deref() == Some(name.as_str()))
        {
            Some(t) => {
                let want_port = desired.local_port.to_string();
                let drift = t.field("local_ip") != Some(desired.local_ip.as_str())
                    || t.field("local_port") != Some(want_port.as_str());
                if drift {
                    require_self_managed(&rd)?;
                    info!(
                        "{}: updating tunnel {} local address -> {}:{}",
                        desired.key, t.id, desired.local_ip, desired.local_port
                    );
                    self.config_tunnel(&t.id, desired).await?;
                }
                let cd = custom_domain_ready(desired, t);
                (t.id.clone(), t.display_address.clone(), cd)
            }
            None => {
                require_self_managed(&rd)?;
                info!(
                    "{}: creating playit tunnel `{}` -> {}:{}",
                    desired.key, name, desired.local_ip, desired.local_port
                );
                let id = self.create_tunnel(desired, &rd.agent_id).await?;
                // Re-fetch so we can report the freshly assigned address.
                let rd2 = self.rundata().await?;
                let created = rd2.tunnels.iter().find(|t| t.id == id);
                let cd = created
                    .map(|t| custom_domain_ready(desired, t))
                    .unwrap_or(false);
                let addr = created.and_then(|t| t.display_address.clone());
                (id, addr, cd)
            }
        };

        Ok(ProvisionedTunnel {
            tunnel_id,
            address: address.unwrap_or_default(),
            custom_domain_ready,
        })
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let name = tunnel_name(key);
        let rd = self.rundata().await?;
        match rd
            .tunnels
            .iter()
            .find(|t| t.name.as_deref() == Some(name.as_str()))
        {
            Some(t) => {
                require_self_managed(&rd)?;
                info!("deleting playit tunnel {} ({name})", t.id);
                self.delete_tunnel(&t.id).await
            }
            None => {
                info!("no playit tunnel named {name} to delete (already gone)");
                Ok(())
            }
        }
    }
}

/// Guard: writes only work for a self-managed agent.
fn require_self_managed(rd: &AgentRunDataV1) -> Result<()> {
    if rd.permissions.is_self_managed {
        Ok(())
    } else {
        Err(Error::Provider(
            "this playit agent is not self-managed, so it can't create or modify tunnels. Claim a \
             self-managed agent (agent_type=self-managed) and give the operator its key via \
             PLAYIT_AGENT_KEY — see CLAUDE.md 'The way forward'."
                .to_string(),
        ))
    }
}

/// Whether the tunnel's current public address already is the requested custom
/// domain. Automatic *attachment* isn't implemented (the playit domains-set
/// endpoint isn't public); attach it once in the dashboard and this reports it.
fn custom_domain_ready(desired: &DesiredTunnel, tunnel: &AgentTunnelV1) -> bool {
    let Some(cd) = desired.custom_domain.as_deref() else {
        return false;
    };
    let host = tunnel
        .display_address
        .as_deref()
        .map(|a| a.split(':').next().unwrap_or(a));
    if host == Some(cd) {
        return true;
    }
    warn!(
        "{}: custom domain `{cd}` is requested but not attached to tunnel {}. Automatic attachment \
         isn't supported yet (the playit domains endpoint isn't public); attach it once in the \
         dashboard (tunnel → Change domain) and the operator will report it as ready.",
        desired.key, tunnel.id
    );
    false
}

fn tunnel_name(key: &str) -> String {
    format!("k8s/{key}")
}

fn build_protocol(desired: &DesiredTunnel) -> ProtocolCreate {
    // A tunnel type (e.g. `https`) overrides the raw protocol/port-count.
    if let Some(tt) = desired.tunnel_type.as_deref() {
        return ProtocolCreate::TunnelType(tt.to_string());
    }
    let port_type = match desired.protocol {
        Protocol::Tcp => "tcp",
        Protocol::Udp => "udp",
        Protocol::Both => "both",
    };
    ProtocolCreate::RawPorts(RawPorts {
        port_type: port_type.to_string(),
        port_count: desired.port_count.max(1),
        software_description: "playit-operator".to_string(),
    })
}

fn local_config(ip: &str, port: u16) -> AgentTunnelConfig {
    AgentTunnelConfig {
        fields: vec![
            AgentTunnelAttr {
                name: "local_ip".to_string(),
                value: ip.to_string(),
            },
            AgentTunnelAttr {
                name: "local_port".to_string(),
                value: port.to_string(),
            },
        ],
    }
}

// --- request bodies (V1) ----------------------------------------------------

#[derive(Serialize)]
struct Empty {}

#[derive(Serialize)]
struct ReqTunnelsCreateV1 {
    name: String,
    protocol: ProtocolCreate,
    origin: OriginCreate,
    endpoint: EndpointCreate,
    enabled: bool,
    firewall_id: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "type", content = "details")]
enum ProtocolCreate {
    /// A named playit tunnel type, e.g. `https`, `minecraft-java`.
    #[serde(rename = "tunnel-type")]
    TunnelType(String),
    /// Plain TCP/UDP port allocation.
    #[serde(rename = "raw-ports")]
    RawPorts(RawPorts),
}

#[derive(Serialize)]
struct RawPorts {
    port_type: String,
    port_count: u16,
    software_description: String,
}

#[derive(Serialize)]
#[serde(tag = "type", content = "data")]
enum OriginCreate {
    #[serde(rename = "agent")]
    Agent(AgentOrigin),
}

#[derive(Serialize)]
struct AgentOrigin {
    agent_id: Option<String>,
    config: AgentTunnelConfig,
}

#[derive(Serialize)]
#[serde(tag = "type", content = "details")]
enum EndpointCreate {
    #[serde(rename = "region")]
    Region(UseAllocRegion),
}

#[derive(Serialize)]
struct UseAllocRegion {
    region: String,
    port: Option<u16>,
}

#[derive(Serialize)]
struct ReqTunnelsConfigV1 {
    tunnel_id: String,
    new_agent_id: Option<String>,
    new_config: Option<AgentTunnelConfig>,
}

#[derive(Serialize)]
struct ReqDelete {
    tunnel_id: String,
}

/// Agent tunnel config — a schema-based `{fields: [{name, value}]}` blob. Used
/// both when writing (create/config) and reading (rundata).
#[derive(Serialize, Deserialize, Clone, Default)]
struct AgentTunnelConfig {
    #[serde(default)]
    fields: Vec<AgentTunnelAttr>,
}

#[derive(Serialize, Deserialize, Clone)]
struct AgentTunnelAttr {
    name: String,
    value: String,
}

// --- response bodies (V1, only the fields we use) ---------------------------

#[derive(Deserialize)]
struct ObjectId {
    id: String,
}

#[derive(Deserialize)]
struct AgentRunDataV1 {
    agent_id: String,
    #[serde(default)]
    tunnels: Vec<AgentTunnelV1>,
    permissions: AgentPermissions,
}

#[derive(Deserialize)]
struct AgentPermissions {
    is_self_managed: bool,
}

#[derive(Deserialize)]
struct AgentTunnelV1 {
    id: String,
    #[serde(default)]
    name: Option<String>,
    /// Best public address, e.g. `host:port` or a custom domain.
    #[serde(default)]
    display_address: Option<String>,
    #[serde(default)]
    agent_config: AgentTunnelConfig,
}

impl AgentTunnelV1 {
    fn field(&self, name: &str) -> Option<&str> {
        self.agent_config
            .fields
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.value.as_str())
    }
}
