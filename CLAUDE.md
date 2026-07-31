# CLAUDE.md

Guidance for Claude Code (and humans) working in this repo. Read this first — it
captures where the project is, what's been learned about the playit.gg API, and
what's left.

## What this is

`playit-operator` is a Kubernetes operator (Rust, [`kube-rs`](https://kube.rs))
that reconciles a `PlayitTunnel` custom resource into a tunnel on
[playit.gg](https://playit.gg), using a **self-managed playit agent**. It's the
playit analogue of a Cloudflare-Tunnel ingress controller: expose in-cluster
services to the internet declaratively instead of clicking around the playit
dashboard.

**Key architectural fact:** playit is a **layer-4 (TCP/UDP) port allocator**, not
an L7 HTTP router. Each `PlayitTunnel` maps *one* public address to *one* Service
port. The operator's sweet spot is TCP/UDP services. It's closer to `external-dns`
than to an HTTP ingress controller: it reconciles Kubernetes objects into
provider-side API state. **HTTPS web apps are a special case the operator can't
fully automate** — see [HTTPS](#https-web-apps-not-operator-automatable).

- CRD group/version: `playit-operator.io/v1alpha1`, kind `PlayitTunnel`
- License: MIT. Image: `ghcr.io/erikandreasklokk/playit-operator:main`

## Status: done & deployed ✅

The operator is **finished and running in the homelab** (verified end-to-end:
create → update → delete of real tunnels).

| Area | State |
| --- | --- |
| Control loop (watch, finalizer, status, Service resolution) | ✅ |
| Dry-run provider (default, no creds) | ✅ |
| Real provider: self-managed **V1** API (`list`/`create`/`config`/`delete`) | ✅ verified live |
| Idempotent create + settled status (no dup tunnels / reconcile loop) | ✅ (both were bugs; fixed) |
| Claim helper (`cargo run --bin claim`) | ✅ |
| CI (fmt, clippy `-D warnings`, build, test, CRD drift) + GHCR image | ✅ |
| Deployed to homelab GitOps (`homelab/applications/playit-operator/`) | ✅ |
| Custom-domain *detection* (report + use in address) | ✅ |
| Custom-domain *auto-attach* | ❌ not possible via agent key (account-level) |
| HTTPS tunnels via the operator | ❌ not possible (needs a domain-bound gateway); use Caddy — see below |

## Architecture / layout

```
src/
├── lib.rs, main.rs        # entrypoint: logging, credential selection, start controller
├── bin/crdgen.rs          # prints the CRD YAML (source of truth is crd.rs)
├── bin/claim.rs           # claim flow → prints a self-managed agent secret key
├── crd.rs                 # PlayitTunnel spec/status (the CRD)
├── controller.rs          # reconcile loop: finalizer, Service→ClusterIP, status patch
├── error.rs
└── provider/
    ├── mod.rs             # TunnelProvider trait + DesiredTunnel/ProvisionedTunnel
    ├── dryrun.rs          # DryRunProvider — logs intended calls, fake address
    └── playit.rs          # PlayitProvider — the real V1 API client
deploy/                    # crd.yaml (generated), rbac.yaml, deployment.yaml, sample-tunnel.yaml
```

The reconcile loop depends only on the `TunnelProvider` trait, so the API access
is swappable and the loop is testable via `DryRunProvider` without credentials.

**How it runs:** a **self-managed playit agent** (data plane) and the operator
(control plane) share one self-managed agent key. The agent forwards traffic; the
operator creates/manages tunnels via the API pointing at that agent. Both are
GitOps'd in `homelab/applications/playit-operator/` (agent, operator, CRD, sealed
key).

## The playit.gg V1 API (what actually works)

⚠️ **Trust the official [`playit-minecraft-plugin`](https://github.com/playit-cloud/playit-minecraft-plugin)
Java models, NOT the `playit-agent` Rust `api.rs`** — the Rust source is a
*different API version* (`ports`/`alloc`/`content="data|details"` differ) and
sending its shapes gives `"failed to parse body"`. The shapes below are the live
ones, verified end-to-end.

- Base URL `https://api.playit.gg`. RPC-style: `POST /<path>` + JSON body.
- Auth header `Authorization: Agent-Key <secret>` (a self-managed agent key).
- Envelope: `{"status":"success","data":<T>}` / `{"status":"error","data":{type,message}}`
  (and `{"status":"fail","data":"<VariantName>"}` for typed failures).

| Path | Purpose |
| --- | --- |
| `/v1/agents/rundata` | `{}` → `{agent_id, permissions:{is_self_managed,...}, tunnels}` — used for agent_id + self-managed check only |
| `/v1/tunnels/list` | `{}` → tunnels (id, name, `origin.details.config_data.fields`, `connect_addresses`). **Use this for enumeration** — immediately consistent |
| `/v1/tunnels/create` | create (shape below) → `{id}` |
| `/v1/tunnels/config` | `{tunnel_id, new_agent_id?, new_config:{fields:[{name,value}]}}` — update local config |
| `/tunnels/delete` | `{tunnel_id}` — works for self-managed (V1 has no delete) |
| `/claim/setup`, `/claim/exchange` | claim flow (no auth) → self-managed key |

**Create body** (`/v1/tunnels/create`, exact fields — see `playit.rs`):
```json
{
  "name": "k8s/<ns>/<name>",
  "protocol": {"type":"raw-ports","details":{"port_type":"tcp","port_count":1,"software_description":"playit-operator"}},
  "origin":   {"type":"agent","data":{"agent_id":"<uuid>","config":{"fields":[{"name":"local_ip","value":"<ClusterIP>"},{"name":"local_port","value":"2283"}]}}},
  "endpoint": {"type":"region","details":{"region":"global","port":null}},
  "enabled": true,
  "firewall_id": null
}
```
Notes that cost hours: field is **`protocol`** not `ports`; **`endpoint`** not
`alloc`; origin content key is **`data`** not `details`; `raw-ports` **requires
`software_description`**; `protocol` can instead be `{"type":"tunnel-type","details":"https"}`.

## Credentials

`PLAYIT_PROVIDER=playit` + `PLAYIT_AGENT_KEY=<self-managed agent secret>`. The
agent **must be self-managed** (`permissions.is_self_managed: true`) or writes are
rejected — the operator surfaces a clear error. Default (no `PLAYIT_PROVIDER`) is
the safe dry-run provider. `PLAYIT_API_KEY` exists in the code but is **dead** —
playit cancelled account API keys (abuse, confirmed by support 2026).

Get a self-managed key: **`cargo run --bin claim`** → open the printed
`playit.gg/claim/<code>` URL, Accept in a browser → it prints the secret to
stdout. Then a running playit agent (same key as `SECRET_KEY`) provides the data
plane.

## HTTPS web apps (not operator-automatable)

Two hard walls make HTTPS a **dashboard + Caddy** job, not an operator one:

1. An HTTPS tunnel needs a `gateway` endpoint bound to a **domain**, and domains
   are account-level state agent keys **cannot see** (`/domains/list` is empty for
   agent keys, even after creating a domain). So the agent/operator can't create
   HTTPS tunnels — `EndpointDoesNotSupportProtocol` with a region endpoint.
2. playit `.playit.plus` HTTPS tunnels **do not terminate TLS** — they forward raw
   TCP to the agent, so *you* terminate TLS locally (their docs say "install
   Caddy"). Speaking TLS to the plain-HTTP backend gives `wrong version number`.

**Working recipe (deployed for Immich):** create the HTTPS tunnel in the dashboard
on the self-managed agent → run a small **Caddy** that auto-obtains a Let's
Encrypt cert (via the tunnel) and reverse-proxies to the app → point the tunnel's
`http_port`/`https_port` at Caddy. See `homelab/applications/immich/caddy-tls.yaml`
— it serves Immich at `https://homelab.playit.plus` (no Cloudflare upload cap).
The tunnel's port/IP config lives in playit (dashboard/API), not git; Caddy's
ClusterIP is pinned so that config stays valid.

## What's left / nice-to-haves

- **Custom-domain auto-attach** — not doable with an agent key (account-level).
  The operator *detects* an attached domain (via the tunnel's address) and reports
  it; attaching is manual in the dashboard.
- **HTTPS via the operator** — blocked as above; a `tunnelType: https` CRD field
  exists but can't complete without a gateway. Leave to Caddy.
- Optional: Kubernetes Events, richer status conditions, driving tunnels from
  annotated `Service` objects (`loadBalancerClass`) instead of the CRD.

## Development workflow

Everything runs in CI on Linux. **On a Linux dev env (e.g. the homelab code-server
container) build/check locally** and mirror CI before pushing:
```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings   # CI treats warnings as errors
cargo build --all-targets && cargo test
cargo run --bin crdgen > deploy/crd.yaml    # after any crd.rs change (CI checks drift)
cargo run --bin playit-operator             # dry-run by default; PLAYIT_PROVIDER=playit PLAYIT_AGENT_KEY=... for real
```
Push to `main` → CI + GHCR image build. The homelab runs the `:main` tag with
`imagePullPolicy: Always`; roll out with `kubectl rollout restart deployment/playit-operator -n playit-operator`.

## Gotchas / lessons learned

- **Self-managed vs assignable is the whole ballgame.** Only self-managed agents
  can write tunnels (V1 API). Check `permissions.is_self_managed`.
- **`AgentVersionTooOld`** on create until a real self-managed agent is *running*
  (it registers a version over the control protocol). Deploy the agent first.
- **Idempotency:** enumerate tunnels via `/v1/tunnels/list` (immediately
  consistent), NOT `/v1/agents/rundata` (lags a just-created tunnel → it once
  created **21 duplicate** tunnels). And only patch `.status` when it changed, or a
  status write re-triggers reconcile in a tight loop. Both fixed; don't regress.
- **`/domains/list` is empty for agent keys** — can't use it for domain/gateway
  discovery.
- **The playit-cli has no tunnel commands** — tunnels are API/dashboard only.
- **curl testing:** send JSON inline or a **BOM-less** file — a UTF-8 BOM →
  `"failed to parse body"`. (PowerShell 5.1 `-Encoding utf8` adds a BOM; use
  `[IO.File]::WriteAllText(path, json, (New-Object Text.UTF8Encoding($false)))`.)
- Runtime image is `gcr.io/distroless/cc-debian12:nonroot`; no shell/apt.
