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
| Real provider: `list` tunnels | ✅ done, verified live (read) |
| Real provider: `create` / `update` / `delete` | ⚠️ implemented, **untested** (needs a write credential — see Blocker) |
| Dual credential (agent key + API key) | ✅ done |
| Custom domains: detect / report / use in address | ✅ done |
| Custom domains: **automatic attach** | ❌ blocked (endpoint not public + write credential) |
| CI (fmt, clippy `-D warnings`, build, test, CRD drift) | ✅ green |
| Docker image publish to GHCR | ✅ green |

**One thing gates the rest: playit account API keys.** See
[The blocker](#the-blocker-account-api-keys).

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

### 🔑 The single most important finding

**playit *agent keys are read-only* for account/tunnel operations.** Verified
live: `/agents/rundata` and `/tunnels/list` return `200`, but `/tunnels/create`
returns `{"type":"auth","message":"NotAllowedWithReadOnly"}`. So the agent key
you already have (the agent's `SECRET_KEY`) **cannot create or modify tunnels or
attach domains** — it can only read. Writes require a **write-capable account API
key** (`Api-Key`).

## Credentials & provider selection

`PLAYIT_PROVIDER` selects the backend:

- unset / `dry-run` (default): logs intended calls, returns a fake address, no creds.
- `playit`: uses the real API. Requires a credential:
  - `PLAYIT_API_KEY` → `Api-Key` header → **write-capable** (preferred).
  - `PLAYIT_AGENT_KEY` → `Agent-Key` header → **read-only** (same value as the
    agent `SECRET_KEY`); listing works, writes fail with a clear error.

Modelled by the `PlayitCredential` enum in `provider/playit.rs`. Adding the API
key when it becomes available is a config-only change — no code edit.

## The blocker: account API keys

Both remaining write features — **tunnel create/update/delete** and **custom
domain attach** — need a write-capable account API key. At the time of writing,
account API keys are **not available** on the maintainer's account (the
`Account → API Keys` page exists but is empty, no "create" button). The
maintainer is checking with playit (Discord) whether/when they can be enabled.

Until then:
- The **read** paths (list, detect attached domains, report status) work with the
  agent key.
- The **write** paths are implemented from the verified API types but are
  **untested end-to-end** and will return a clear "credential is read-only" error
  if run with an agent key.

There is a fragile alternative (the dashboard's `__Secure-WebAuth` session
cookie) that was **deliberately not implemented** — it expires, is unofficial,
and adds work to remove later.

## What's left to do

1. **Verify the write path** once an API key exists: set `PLAYIT_PROVIDER=playit`
   + `PLAYIT_API_KEY`, then exercise create → update → delete against a real
   account (create+delete a throwaway `PlayitTunnel`). Confirm the exact
   `alloc`/region behaviour (currently defaults to region `"global"`; untested).
2. **Custom-domain auto-attach** — the missing piece. The "set tunnel domain"
   endpoint is **not** in the agent's public API surface. To finish:
   - Discover the endpoint by watching the dashboard's Network tab while attaching
     a domain to a tunnel (needs an account that can do it), then
   - Implement it in `custom_domain_status`'s TODO spot in `playit.rs` (attach
     `domain_id` → `tunnel_id`), gated behind a write credential.
3. **Homelab deployment / GitOps** — the `deploy/` manifests exist; wiring this
   into an ArgoCD app (reusing the existing agent token secret for
   `PLAYIT_AGENT_KEY`, or an API key secret for `PLAYIT_API_KEY`) is a future step.
4. Nice-to-haves: emit Kubernetes Events, richer status conditions, optionally
   drive tunnels from annotated `Service` objects (`loadBalancerClass`) in
   addition to the CRD.

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

- **Agent keys are read-only** (see above) — the defining constraint of the whole
  project right now.
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
