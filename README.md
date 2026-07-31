# playit-operator

A Kubernetes operator that turns a `PlayitTunnel` custom resource into a live
[playit.gg](https://playit.gg) tunnel — the playit equivalent of running a
Cloudflare Tunnel ingress controller, so you can expose in-cluster services to
the internet declaratively instead of clicking around the playit dashboard.

> **Status: early / alpha.** The full control loop (watch, finalizers, status,
> Service resolution) works today. The real playit.gg API provider is
> implemented (`list`/`create`/`update`/`delete`), but the **write path is
> untested end-to-end** because it needs a write-capable account API key — see
> [Credentials](#credentials-playit_providerplayit) and [Roadmap](#roadmap). The
> **dry-run** provider is the default and needs no credentials. Contributions
> welcome.

## Why this exists

Cloudflare Tunnel does layer-7 HTTP host routing: one tunnel carries
`*.example.com` and adding an app is basically free. **playit is different — it's
a layer-4 (TCP/UDP) port allocator.** Each tunnel maps one public address to one
port, which is exactly what you want for game servers, SSH, and other non-HTTP
services that Cloudflare Tunnel handles awkwardly.

So this operator's job is: *for each `PlayitTunnel`, make sure a playit tunnel
exists, pointed at the right `Service`, and (later) with a custom domain
attached.* Conceptually it's closer to [external-dns](https://github.com/kubernetes-sigs/external-dns)
than to an HTTP ingress controller — it reconciles Kubernetes objects into
provider-side API state.

## How it works

```
PlayitTunnel CR  ──watch──►  operator  ──TunnelProvider──►  playit.gg API
     │                          │                               │
     │                    resolve Service                 create/update
     │                    (ClusterIP:port)                 tunnel + domain
     ▼                          │                               │
  .status  ◄──────patch─────────┘◄──────── assigned address ────┘
```

The playit **agent** itself is unchanged — you keep running the official
`ghcr.io/playit-cloud/playit-agent` in your cluster. The agent pulls its tunnel
config from the playit control plane (`AgentRunDataV1`), so when the operator
creates or updates a tunnel via the API, the running agent picks it up **without
a restart**. The operator never has to touch the agent.

Provider selection is via environment variable:

| `PLAYIT_PROVIDER` | Behaviour |
| --- | --- |
| unset / `dry-run` (default) | Logs the API calls it *would* make and returns a deterministic fake address. Safe without credentials. |
| `playit` | Uses the real `https://api.playit.gg`. Requires a credential (below). |

### Credentials (`PLAYIT_PROVIDER=playit`)

| Env var | Auth header | Capability |
| --- | --- | --- |
| `PLAYIT_API_KEY` | `Api-Key <v>` | **Write** — create/update/delete tunnels. Preferred. |
| `PLAYIT_AGENT_KEY` | `Agent-Key <v>` | **Read-only** — listing works, but playit rejects writes with `NotAllowedWithReadOnly`. Same value as the agent's `SECRET_KEY`. |

> ⚠️ **Heads up on credentials.** playit **agent keys are read-only** for tunnel
> management, so they can't actually create tunnels — the operator will surface a
> clear error until a write-capable key is supplied. Account **API keys** are the
> write path, but at time of writing they aren't available on all accounts (the
> account "API Keys" page may be empty). The operator already speaks both, so the
> day you can mint an API key, just set `PLAYIT_API_KEY` — no code change.

## Custom resource

```yaml
apiVersion: playit-operator.io/v1alpha1
kind: PlayitTunnel
metadata:
  name: minecraft
  namespace: default
spec:
  protocol: tcp          # tcp | udp | both
  serviceName: minecraft # target Service in the same namespace
  port: 25565            # port on that Service
  # portCount: 1         # allocate a range of consecutive ports
  # region: eu-central   # optional playit region preference
  # customDomain: play.example.com  # requires playit Premium (roadmap)
```

The operator writes results back to `.status`:

```yaml
status:
  phase: Ready
  tunnelId: "..."
  address: "147.185.221.x:25565"
  observedGeneration: 1
```

## Install

```bash
kubectl apply -f deploy/crd.yaml
kubectl apply -f deploy/rbac.yaml
kubectl apply -f deploy/deployment.yaml
# then create PlayitTunnel resources, e.g.:
kubectl apply -f deploy/sample-tunnel.yaml
```

To enable the real API later, set `PLAYIT_PROVIDER=playit` and mount a
`PLAYIT_API_KEY` (see the commented block in `deploy/deployment.yaml`).

## Development

```bash
cargo build                      # build operator + crdgen
cargo run --bin crdgen           # print the CRD (source of truth is src/crd.rs)
cargo run --bin playit-operator  # run against your current kubeconfig context
```

Requires a C toolchain for the linker (on Windows: the Visual Studio C++ build
tools, or build in the provided Docker image). CI builds on Linux.

## Roadmap

- [x] Wire `PlayitProvider` to the real playit.gg API — `tunnels/list`,
      `tunnels/create`, `tunnels/update`, `tunnels/delete`, with `origin=agent`
      pointed at the Service ClusterIP. Auth via `Agent-Key`/`Api-Key`.
- [ ] Verify the write path end-to-end against a live account (blocked on a
      write-capable API key — agent keys are read-only; create/update/delete are
      implemented from the API types but untested against a real write).
- [~] **Custom domains** — *partially implemented*. The operator now detects
      whether `spec.customDomain` is attached to the tunnel (via the tunnel's
      `domain` field), reports it in `status.customDomainReady`, and uses the
      custom domain as the public `status.address`. **Automatic attachment** is
      still pending: the playit "set tunnel domain" endpoint isn't in the public
      API (only `/domains/list`, which returns empty for agent keys) and it's a
      write op, so today you attach the domain once in the dashboard and the
      operator keeps it. Full auto-attach needs the endpoint + a write key.
- [ ] Optional: drive tunnels straight from annotated `Service` objects
      (`loadBalancerClass: playit-operator.io/tunnel`) in addition to the CRD.
- [ ] Optional: emit Kubernetes Events and richer status conditions.

## License

[MIT](LICENSE) © Erik Andreas Klokk. Use it, fork it, ship it — no warranty.
