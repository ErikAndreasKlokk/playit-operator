//! The real provider: talks to the playit.gg API (`https://api.playit.gg`).
//!
//! The API is RPC-style — every call is `POST /<path>` with a JSON body and an
//! `Authorization: <Kind>-Key <secret>` header, returning an envelope of the
//! form `{"status":"success","data":…}` or `{"status":"error","data":{…}}`.
//!
//! Auth is modelled by [`PlayitCredential`]. Note: an **agent** key is read-only
//! for account/tunnel operations — playit rejects create/update/delete with
//! `NotAllowedWithReadOnly`. Writes require an account **API key**, which this
//! provider is wired for so it works the moment playit offers one (no code
//! change; just set `PLAYIT_API_KEY`).

use async_trait::async_trait;
use reqwest::header::AUTHORIZATION;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use tracing::{info, warn};

use super::{DesiredTunnel, ProvisionedTunnel, TunnelProvider};
use crate::crd::Protocol;
use crate::error::{Error, Result};

const API_BASE: &str = "https://api.playit.gg";

/// A credential for the playit.gg API.
#[derive(Clone)]
pub enum PlayitCredential {
    /// Agent secret key (the value used as the agent's `SECRET_KEY`). Read-only
    /// for account/tunnel operations: fine for listing, but the API rejects
    /// create/update/delete with `NotAllowedWithReadOnly`.
    AgentKey(String),
    /// Account API key. Write-capable. Not offered on every account yet (the
    /// account "API Keys" page may be empty), but wired up here so enabling it
    /// later is a config-only change.
    ApiKey(String),
}

impl PlayitCredential {
    fn header_value(&self) -> String {
        match self {
            PlayitCredential::AgentKey(v) => format!("Agent-Key {}", v.trim()),
            PlayitCredential::ApiKey(v) => format!("Api-Key {}", v.trim()),
        }
    }

    /// Whether this credential can only read (agent keys can't write tunnels).
    fn is_read_only(&self) -> bool {
        matches!(self, PlayitCredential::AgentKey(_))
    }
}

/// Provider backed by the live playit.gg API.
pub struct PlayitProvider {
    http: reqwest::Client,
    auth_header: String,
    read_only: bool,
    agent_id: OnceLock<String>,
}

impl PlayitProvider {
    /// Construct a provider that authenticates with the given credential.
    pub fn new(credential: PlayitCredential) -> Self {
        Self {
            http: reqwest::Client::new(),
            auth_header: credential.header_value(),
            read_only: credential.is_read_only(),
            agent_id: OnceLock::new(),
        }
    }

    /// POST `body` to `path` and return the `data` field, translating the
    /// playit error envelope into a [`Error::Provider`].
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
        if kind == "auth" && msg == "NotAllowedWithReadOnly" {
            Err(Error::Provider(
                "playit rejected the write: this credential is read-only (agent keys cannot \
                 create or modify tunnels). Supply a write-capable account API key via \
                 PLAYIT_API_KEY."
                    .to_string(),
            ))
        } else {
            Err(Error::Provider(format!("playit {path}: {kind} — {msg}")))
        }
    }

    /// Like [`Self::call_raw`] but deserializes `data` into `R`.
    async fn call<B: Serialize, R: DeserializeOwned>(&self, path: &str, body: &B) -> Result<R> {
        let data = self.call_raw(path, body).await?;
        serde_json::from_value(data)
            .map_err(|e| Error::Provider(format!("decoding {path} data: {e}")))
    }

    /// The id of the agent this credential belongs to, discovered once from
    /// `/agents/rundata` and cached.
    async fn agent_id(&self) -> Result<String> {
        if let Some(id) = self.agent_id.get() {
            return Ok(id.clone());
        }
        let data: AgentRunData = self.call("/agents/rundata", &Empty {}).await?;
        let _ = self.agent_id.set(data.agent_id.clone());
        Ok(data.agent_id)
    }

    async fn list_tunnels(&self) -> Result<Vec<AccountTunnel>> {
        let req = ReqList {
            tunnel_id: None,
            agent_id: None,
        };
        let data: AccountTunnels = self.call("/tunnels/list", &req).await?;
        Ok(data.tunnels)
    }

    async fn create_tunnel(
        &self,
        name: &str,
        desired: &DesiredTunnel,
        agent_id: &str,
    ) -> Result<String> {
        // Default to the "global" network when no region is requested. Override
        // per-tunnel with `spec.region`. (Untested against a live write until an
        // account API key is available — agent keys are read-only.)
        let region = desired
            .region
            .clone()
            .unwrap_or_else(|| "global".to_string());
        let req = ReqCreate {
            name: name.to_string(),
            tunnel_type: None,
            port_type: port_type_str(desired.protocol).to_string(),
            port_count: desired.port_count.max(1),
            origin: OriginCreate::Agent {
                agent_id: agent_id.to_string(),
                local_ip: desired.local_ip.clone(),
                local_port: Some(desired.local_port),
            },
            enabled: true,
            alloc: Some(AllocCreate::Region { region }),
            firewall_id: None,
            proxy_protocol: None,
        };
        let obj: ObjectId = self.call("/tunnels/create", &req).await?;
        Ok(obj.id)
    }

    async fn update_tunnel(
        &self,
        tunnel_id: &str,
        desired: &DesiredTunnel,
        agent_id: &str,
    ) -> Result<()> {
        let req = ReqUpdate {
            tunnel_id: tunnel_id.to_string(),
            local_ip: desired.local_ip.clone(),
            local_port: Some(desired.local_port),
            agent_id: Some(agent_id.to_string()),
            enabled: true,
        };
        self.call_raw("/tunnels/update", &req).await?;
        Ok(())
    }

    async fn delete_tunnel(&self, tunnel_id: &str) -> Result<()> {
        let req = ReqDelete {
            tunnel_id: tunnel_id.to_string(),
        };
        self.call_raw("/tunnels/delete", &req).await?;
        Ok(())
    }

    /// Whether the requested custom domain is attached to `tunnel`, returning
    /// `(ready, host_override)`.
    ///
    /// Automatic *attachment* isn't wired up yet: the playit "set tunnel domain"
    /// endpoint isn't in the public API and would need a write-capable
    /// credential. So when a domain is requested but not yet attached, this warns
    /// and reports `ready = false`. Attach it once in the dashboard
    /// (tunnel → Change domain); the operator then reports it ready and uses it
    /// as the public address.
    fn custom_domain_status(
        &self,
        desired: &DesiredTunnel,
        tunnel: &AccountTunnel,
    ) -> (bool, Option<String>) {
        let Some(cd) = desired.custom_domain.as_deref() else {
            return (false, None);
        };
        if tunnel.domain.as_ref().map(|d| d.name.as_str()) == Some(cd) {
            return (true, Some(cd.to_string()));
        }
        warn!(
            "{}: custom domain `{cd}` is requested but not attached to tunnel {}. Automatic \
             attachment isn't supported yet (the playit domains endpoint isn't public and needs a \
             write-capable API key); attach it once in the dashboard (tunnel → Change domain) and \
             the operator will report it as ready.",
            desired.key, tunnel.id
        );
        (false, None)
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
        let agent_id = self.agent_id().await?;

        // Get-or-create the tunnel, then report on it uniformly below.
        let tunnel = match self
            .list_tunnels()
            .await?
            .into_iter()
            .find(|t| t.name == name)
        {
            Some(t) => {
                let addr_matches = t.origin.data.local_ip.as_deref()
                    == Some(desired.local_ip.as_str())
                    && t.origin.data.local_port == Some(desired.local_port);
                if !addr_matches {
                    if self.read_only {
                        return Err(read_only_write_error());
                    }
                    info!(
                        "{}: updating tunnel {} local address -> {}:{}",
                        desired.key, t.id, desired.local_ip, desired.local_port
                    );
                    self.update_tunnel(&t.id, desired, &agent_id).await?;
                }
                t
            }
            None => {
                if self.read_only {
                    return Err(read_only_write_error());
                }
                info!(
                    "{}: creating playit tunnel `{}` -> {}:{}",
                    desired.key, name, desired.local_ip, desired.local_port
                );
                let id = self.create_tunnel(&name, desired, &agent_id).await?;
                self.list_tunnels()
                    .await?
                    .into_iter()
                    .find(|t| t.id == id)
                    .ok_or_else(|| {
                        Error::Provider("created tunnel not found when re-listing".to_string())
                    })?
            }
        };

        let (custom_domain_ready, cd_host) = self.custom_domain_status(desired, &tunnel);
        let host = cd_host.or_else(|| tunnel.default_host());
        let address = match (host, tunnel.port()) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            (Some(h), None) => h,
            _ => String::new(),
        };
        Ok(ProvisionedTunnel {
            tunnel_id: tunnel.id.clone(),
            address,
            custom_domain_ready,
        })
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let name = tunnel_name(key);
        match self
            .list_tunnels()
            .await?
            .into_iter()
            .find(|t| t.name == name)
        {
            Some(t) => {
                if self.read_only {
                    return Err(read_only_write_error());
                }
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

fn tunnel_name(key: &str) -> String {
    format!("k8s/{key}")
}

fn port_type_str(p: Protocol) -> &'static str {
    match p {
        Protocol::Tcp => "tcp",
        Protocol::Udp => "udp",
        Protocol::Both => "both",
    }
}

fn read_only_write_error() -> Error {
    Error::Provider(
        "credential is read-only (agent keys cannot create/modify tunnels); set a write-capable \
         account API key via PLAYIT_API_KEY"
            .to_string(),
    )
}

// --- request bodies ---------------------------------------------------------

#[derive(Serialize)]
struct Empty {}

#[derive(Serialize)]
struct ReqList {
    tunnel_id: Option<String>,
    agent_id: Option<String>,
}

#[derive(Serialize)]
struct ReqCreate {
    name: String,
    tunnel_type: Option<String>,
    port_type: String,
    port_count: u16,
    origin: OriginCreate,
    enabled: bool,
    alloc: Option<AllocCreate>,
    firewall_id: Option<String>,
    proxy_protocol: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "type", content = "data", rename_all = "lowercase")]
enum OriginCreate {
    Agent {
        agent_id: String,
        local_ip: String,
        local_port: Option<u16>,
    },
}

#[derive(Serialize)]
#[serde(tag = "type", content = "details", rename_all = "kebab-case")]
enum AllocCreate {
    Region { region: String },
}

#[derive(Serialize)]
struct ReqUpdate {
    tunnel_id: String,
    local_ip: String,
    local_port: Option<u16>,
    agent_id: Option<String>,
    enabled: bool,
}

#[derive(Serialize)]
struct ReqDelete {
    tunnel_id: String,
}

// --- response bodies (only the fields we use) -------------------------------

#[derive(Deserialize)]
struct ObjectId {
    id: String,
}

#[derive(Deserialize)]
struct AgentRunData {
    agent_id: String,
}

#[derive(Deserialize)]
struct AccountTunnels {
    tunnels: Vec<AccountTunnel>,
}

#[derive(Deserialize)]
struct AccountTunnel {
    id: String,
    name: String,
    alloc: TunnelAlloc,
    origin: TunnelOrigin,
    #[serde(default)]
    domain: Option<TunnelDomain>,
}

impl AccountTunnel {
    /// The public port allocated to the tunnel, if allocated.
    fn port(&self) -> Option<u16> {
        self.alloc.data.as_ref().and_then(|d| d.port_start)
    }

    /// The best public hostname for the tunnel: its attached custom domain if
    /// present, otherwise the playit-assigned domain.
    fn default_host(&self) -> Option<String> {
        self.domain.as_ref().map(|d| d.name.clone()).or_else(|| {
            self.alloc
                .data
                .as_ref()
                .and_then(|d| d.assigned_domain.clone())
        })
    }
}

#[derive(Deserialize)]
struct TunnelAlloc {
    #[serde(default)]
    data: Option<AllocData>,
}

#[derive(Deserialize)]
struct AllocData {
    #[serde(default)]
    assigned_domain: Option<String>,
    #[serde(default)]
    port_start: Option<u16>,
}

#[derive(Deserialize)]
struct TunnelOrigin {
    data: OriginData,
}

#[derive(Deserialize)]
struct OriginData {
    #[serde(default)]
    local_ip: Option<String>,
    #[serde(default)]
    local_port: Option<u16>,
}

#[derive(Deserialize)]
struct TunnelDomain {
    name: String,
}
