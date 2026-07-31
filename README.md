# playit-operator

A Kubernetes operator that turns a `PlayitTunnel` custom resource into a live
[playit.gg](https://playit.gg) tunnel — the playit equivalent of running a
Cloudflare Tunnel ingress controller, so you can expose in-cluster services to
the internet declaratively instead of clicking around the playit dashboard.

> **Status: early / alpha.** The full control loop (watch, finalizers, status,
> Service resolution) works today against a **dry-run** provider. Talking to the
> real playit.gg API is stubbed behind a trait and is the next milestone — see
> [Roadmap](#roadmap). Contributions welcome.

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
| `playit` | Uses the real `https://api.playit.gg` (requires `PLAYIT_API_KEY`). Not yet implemented. |

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

- [ ] Wire `PlayitProvider` to the real playit.gg API (`tunnels/create`,
      `tunnels/list`, `tunnels/update`, `tunnels/delete`) using an account API key.
- [ ] **Custom domains** — attach a domain to a tunnel (playit Premium). The CR
      field (`spec.customDomain`) and status (`status.customDomainReady`) already
      exist so enabling it is non-breaking.
- [ ] Optional: drive tunnels straight from annotated `Service` objects
      (`loadBalancerClass: playit-operator.io/tunnel`) in addition to the CRD.
- [ ] Optional: emit Kubernetes Events and richer status conditions.

## License

[MIT](LICENSE) © Erik Andreas Klokk. Use it, fork it, ship it — no warranty.
