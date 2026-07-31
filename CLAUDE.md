# CLAUDE.md

Guidance for Claude Code (and humans) working in this repo. Read this first — it
captures where the project is, what's been learned about the playit.gg API, and
what's left to do.

## What this is

`playit-operator` is a Kubernetes operator (Rust, [`kube-rs`](https://kube.rs))
that reconciles a `PlayitTunnel` custom resource into a tunnel on
[playit.gg](https://playit.gg). It's the playit analogue of running a Cloudflare
Tunnel ingress controller: expose in-cluster services to the internet
declaratively instead of clicking around the playit dashboard.

**Key architectural fact:** playit is a **layer-4 (TCP/UDP) port allocator**, not
an L7 HTTP router. Each `PlayitTunnel` maps *one* public address to *one* Service
port. There is no host-header multiplexing (that's what Cloudflare Tunnel already
does well). The operator's sweet spot is TCP/UDP services — game servers, SSH,
etc. Conceptually it's closer to `external-dns` than to an HTTP ingress
controller: it reconciles Kubernetes objects into provider-side API state.

- CRD group/version: `playit-operator.io/v1alpha1`, kind `PlayitTunnel`
- License: MIT. Image: `ghcr.io/erikandreasklokk/playit-operator:main`

## Status at a glance

| Area | State |
| --- | --- |
| Control loop (watch, finalizer, status, Service resolution) | ✅ done |
| Dry-run provider (default, no creds) | ✅ done |
| Real provider: `list` tunnels (account `/tunnels/list`) | ✅ done, verified live (read) |
| Real provider: `create`/`update`/`delete` (account `/tunnels/*`) | ⚠️ implemented but a **dead end** — see below |
| Dual credential (agent key + API key) | ✅ done, but API keys are cancelled (abuse) |
| Custom domains: detect / report / use in address | ✅ done |
| CI (fmt, clippy `-D warnings`, build, test, CRD drift) | ✅ green |
| Docker image publish to GHCR | ✅ green |
| **Self-managed agent + V1 API write path** | 🔜 **the way forward — not yet implemented** |

**Read this before continuing.** The original plan (account API key → account
`/tunnels/*` write endpoints) is **dead**: playit paused API keys indefinitely due
to abuse (confirmed by support, 2026). The account `/tunnels/create` etc. that the
provider currently implements return `NotAllowedWithReadOnly` for agent keys and
there's no key that unlocks them. **The viable path is a self-managed agent using
the V1 API** — see [The way forward](#the-way-forward-self-managed-agent--v1-api).

## Architecture / layout

```
src/
├── lib.rs             # module root
├── main.rs            # entrypoint: logging, credential selection, start controller
├── bin/crdgen.rs      # prints the CRD YAML (source of truth is crd.rs)
├── crd.rs             # PlayitTunnel spec/status types (the CRD)
├── controller.rs      # reconcile loop: finalizer, Service → ClusterIP resolution, status patch
├── error.rs           # Error enum
└── provider/
    ├── mod.rs         # TunnelProvider trait + DesiredTunnel/ProvisionedTunnel
    ├── dryrun.rs      # DryRunProvider — logs intended calls, returns a fake address
    └── playit.rs      # PlayitProvider — the real https://api.playit.gg client
deploy/                # crd.yaml (generated), rbac.yaml, deployment.yaml, sample-tunnel.yaml
.github/workflows/     # ci.yml (checks), docker-publish.yml (GHCR image)
```

The reconcile loop depends only on the `TunnelProvider` trait, so the real API
access is swappable and the loop is testable via `DryRunProvider` without
credentials.

**The playit agent is left untouched.** You keep running the official
`ghcr.io/playit-cloud/playit-agent` in the cluster. The agent pulls its tunnel
config from playit's control plane (`AgentRunData`), so when the operator creates
or updates a tunnel via the API, the running agent picks it up **without a
restart**. The operator never touches the agent — it only drives the API.

## The playit.gg API (what we learned)

Reverse-engineered from `playit-cloud/playit-agent` (`packages/api_client/src/api.rs`)
and **verified against the live API**. There is no official public REST doc, but
there is an official (undocumented) `playit-api-client` Rust crate and a
`playit-api-java` client.

- Base URL: `https://api.playit.gg`. RPC-style: every call is `POST /<path>` with
  a JSON body.
- Auth header: `Authorization: <Kind>-Key <secret>` — either `Agent-Key <v>` or
  `Api-Key <v>`.
- Response envelope: `{"status":"success","data":<T>}` or
  `{"status":"error","data":{"type":"...","message":"..."}}`.

Endpoints the operator uses:

| Path | Purpose | Body | Notes |
| --- | --- | --- | --- |
| `/agents/rundata` | discover our `agent_id` + assigned tunnels | `{}` | read |
| `/tunnels/list` | list account tunnels | `{}` | read |
| `/tunnels/create` | create a tunnel | `ReqTunnelsCreate` | **write** |
| `/tunnels/update` | change local address | `ReqTunnelsUpdate` | **write** |
| `/tunnels/delete` | delete a tunnel | `{tunnel_id}` | **write** |
| `/domains/list` | list account domains | `{}` | read — **returns `[]` for agent keys** (unreliable; don't use for validation) |

Tunnel `create` shape (see `playit.rs` for the exact Rust structs):
`{ name, tunnel_type: null, port_type: "tcp"|"udp"|"both", port_count, origin:
{type:"agent", data:{agent_id, local_ip, local_port}}, enabled: true, alloc:
{type:"region", details:{region:"global"|...}}, firewall_id: null, proxy_protocol:
null }`. `local_ip` is the target Service's **ClusterIP** (the playit agent, which
runs in-cluster, forwards there). A domain attached to a tunnel appears on the
tunnel object as `domain: {id, name}`.

### 🔑 The two most important findings

1. **Account `/tunnels/*` writes need account auth that doesn't exist.** An
   *assignable* agent key (what a normal agent has, `is_self_managed: false`) is
   read-only on the account API: `/agents/rundata` and `/tunnels/list` return
   `200`, but `/tunnels/create` returns
   `{"type":"auth","message":"NotAllowedWithReadOnly"}`. The only thing that would
   unlock these is an account API key — **which playit has cancelled** (abuse).
   So the currently-implemented account-`/tunnels/*` write path is a dead end.

2. **Self-managed agents can create their own tunnels via the V1 API.** An agent
   claimed as `self-managed` (`permissions.is_self_managed: true`) uses
   `/v1/tunnels/create` with *its own agent key* — no account API key. This is how
   the official Minecraft plugin works and it's the path forward.

## Credentials & provider selection

`PLAYIT_PROVIDER` selects the backend:

- unset / `dry-run` (default): logs intended calls, returns a fake address, no creds.
- `playit`: uses the real API. Requires a credential:
  - `PLAYIT_API_KEY` → `Api-Key` header → **write-capable** (preferred).
  - `PLAYIT_AGENT_KEY` → `Agent-Key` header → **read-only** (same value as the
    agent `SECRET_KEY`); listing works, writes fail with a clear error.

Modelled by the `PlayitCredential` enum in `provider/playit.rs`. **Note:** the
`Api-Key` variant is now effectively dead (playit cancelled account API keys). The
real path uses a *self-managed* `Agent-Key` against the **V1** endpoints — the
current provider still targets the account `/tunnels/*` endpoints, so this is the
main pending rework. See [The way forward](#the-way-forward-self-managed-agent--v1-api).

## The way forward: self-managed agent + V1 API

playit **cancelled account API keys indefinitely** (abuse — confirmed by support,
2026), so the `Api-Key`/account-`/tunnels/*` path is dead. Instead, use a
**self-managed agent**. Reference implementation: the official
[`playit-minecraft-plugin`](https://github.com/playit-cloud/playit-minecraft-plugin)
(`PlayitKeysSetup.java` = claim flow, `PlayitManager.java#ensureTunnelExists` =
V1 create).

### 1. Provision a self-managed agent key (one-time, semi-interactive claim)

```
POST /claim/setup    {code, agent_type: "self-managed", version}  → status (UserAccepted / WaitingForUser / ...)
   ↳ user opens https://playit.gg/claim/<code> and accepts
POST /claim/exchange {code}                                       → {secret_key}   # the self-managed agent key
```

`code` is random hex (the plugin uses 8 random bytes). Poll `/claim/setup` until
`UserAccepted`, then `/claim/exchange`. Verify with `/v1/agents/rundata` — it
should show `permissions.is_self_managed: true`.

### 2. Create/manage tunnels with that key via the V1 API

Auth is the same `Agent-Key <secret>` header; the endpoints differ from the
account API:

| Path | Purpose | Body (key fields) |
| --- | --- | --- |
| `/v1/tunnels/list` | list this agent's tunnels | `{}` → `AccountTunnelsV1` |
| `/v1/tunnels/create` | create a tunnel | `ReqTunnelsCreateV1 { ports, origin: {type:"agent", details:{agent_id, config}}, enabled, alloc: {type:"region", details:{region, port:null}}, name, firewall_id:null }` |
| `/v1/tunnels/config` | update local address / agent | `ReqTunnelsConfigV1 { tunnel_id, new_agent_id?, new_config? }` |
| `/tunnels/delete` | delete (V1 has no delete endpoint — reuse account delete or disable) | `{tunnel_id}` — **verify this works for self-managed** |

`origin.details.config` is an `AgentTunnelConfig` — a schema-based `{fields:
[{name:"local_ip", value}, {name:"local_port", value}]}` object (see the
`config_data` in a `/v1/tunnels/list` response). The V1 tunnel/response shapes are
richer than the account API (see `AccountTunnelV1`, `connect_addresses`) — model
only the fields the operator needs.

### 3. Homelab / architecture decision (for the maintainer)

The existing homelab agent is **assignable** (`is_self_managed: false`) and owns
the current tunnels (minecraft, qbittorrent). Options:
- **Run a second, self-managed agent** for operator-created tunnels (cleanest —
  leaves existing tunnels untouched), give the operator *that* agent's key, and
  set the tunnels' `origin.agent_id` to it; or
- Re-claim/replace the existing agent as self-managed (migrates existing tunnels;
  more disruptive).

The operator only needs the self-managed **key** to call the V1 API; a running
self-managed **agent** (same key) handles the data-plane forwarding.

> A fragile alternative (the dashboard `__Secure-WebAuth` session cookie) was
> **deliberately rejected** — expires, unofficial, more work to remove later.

## What's left to do

1. **Rework `PlayitProvider` onto the self-managed V1 path** (the big one). Swap
   the account `/tunnels/*` calls for the V1 endpoints above, using a
   self-managed agent key. Keep the `TunnelProvider` trait and the whole control
   loop as-is — this is a provider-internal change (new request/response structs,
   new paths). The `PlayitCredential::ApiKey` variant can stay as dormant/dead
   scaffolding or be removed.
2. **Claim helper.** Add a way to provision the self-managed key (the
   `/claim/setup` → user-accept → `/claim/exchange` flow). Could be a `crdgen`-style
   helper binary (`cargo run --bin claim`) that prints the claim URL and writes
   the secret, or documented manual `curl` steps. The maintainer must accept the
   claim in a browser once.
3. **Verify write end-to-end** with the self-managed key: create → update
   (`/v1/tunnels/config`) → delete a throwaway `PlayitTunnel`. Confirm delete
   works for self-managed (V1 has no delete endpoint — may need `/tunnels/delete`
   or a disable). Confirm `alloc`/region behaviour.
4. **Custom-domain auto-attach** — still the fuzziest. The "set tunnel domain"
   endpoint isn't in the public API; discover it from the dashboard Network tab,
   then implement at the TODO in `custom_domain_status` (`playit.rs`). May or may
   not be doable with a self-managed agent key — test.
5. **Homelab deployment / GitOps** — `deploy/` manifests exist; wire an ArgoCD app
   that runs a self-managed agent + the operator (operator gets the self-managed
   key via `PLAYIT_AGENT_KEY`). See the architecture decision above.
6. Nice-to-haves: Kubernetes Events, richer status conditions, optionally drive
   tunnels from annotated `Service` objects (`loadBalancerClass`).

## Development workflow

Everything runs in CI on Linux. **On a Linux dev environment (e.g. a homelab
code-server container) you can build and check locally** with the standard Rust
toolchain — mirror CI exactly before pushing:

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings   # CI treats warnings as errors
cargo build --all-targets
cargo test
```

Regenerate the CRD after changing `crd.rs` (CI fails if `deploy/crd.yaml` is
stale — it must be byte-identical to the generator output):

```bash
cargo run --bin crdgen > deploy/crd.yaml
```

Run the operator against your current kube context (dry-run by default, safe):

```bash
cargo run --bin playit-operator
# real API: PLAYIT_PROVIDER=playit PLAYIT_API_KEY=... cargo run --bin playit-operator
```

CI (`.github/workflows/ci.yml`) runs fmt + clippy + build + test + a CRD
drift check. Pushing to `main` also builds and pushes the GHCR image
(`docker-publish.yml`). To watch a run: `gh run watch <id> -R ErikAndreasKlokk/playit-operator`.

## Gotchas / lessons learned

- **Assignable agent keys are read-only** on the account API; **self-managed**
  agent keys can create tunnels via the **V1** API. The agent *type* (set at claim
  time) is the whole ballgame — check `permissions.is_self_managed` in
  `/v1/agents/rundata`. Account API keys are cancelled (abuse).
- `/domains/list` returns `[]` for agent keys even when domains are attached to
  tunnels. Detect attached custom domains via the **tunnel's** `domain` field
  (from `/tunnels/list`), not the domains list.
- Custom domains work fine on the account when attached manually in the dashboard;
  only *automated* attach is missing.
- The playit-cli has **no** tunnel commands — tunnels are managed via the API or
  dashboard, not the agent CLI. The agent only reads its assigned config.
- When testing the API by hand with `curl`, send JSON via `--data` inline or a
  **BOM-less** file — a UTF-8 BOM makes the server reply `"failed to parse body"`.
- The runtime image is `gcr.io/distroless/cc-debian12:nonroot` (uid 65532); no
  shell/apt. Don't reintroduce an `apt-get`/`useradd` step in the Dockerfile.
