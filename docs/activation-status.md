# Activation Status — what's needed to go live

Snapshot as of the initial scaffold + edge deploy. Delete this file once the service is actually running game servers.

## Already live (no action needed)

- **Edge forwarding** is deployed and verified. The Hetzner VPS and R730xd forward the **7000–7010** UDP+TCP range over the `wg0` tunnel to the cluster node (`dell_inspiron`, `10.0.0.226`). Lives in `grizzly-platform` ansible — see that repo's `setup-proxy-vps.yml` / `setup-r730xd.yml` and `ADR-019`. The range forwards to nothing until Agones + a game server exist, which is expected.

## Pending pushes (required to activate)

1. **This repo** — `feat/scaffold-service-structure` is committed locally but unpushed. Merge to `main` and push so the scaffold/design lands on the remote.
2. **grizzly-platform** — `feat/game-server-edge-forwarding` is committed locally but unpushed. Merge to **`master`** and push. Flux watches `master` and auto-applies, so this is what registers the app (`kubernetes/apps/grizzly-gameservers/`). Until then the Flux source/release does not exist in the cluster.

## Still not live even after those pushes (needs implementation)

Pushing the above registers the app with Flux but does **not** make it functional. The Flux `HelmRelease` will sit empty / NotReady because:

- **Agones is not installed** (`cluster/agones/` is a placeholder).
- **`deploy/` is a placeholder chart** with no templates — no bot/agent workload.
- **No guardrails or Kyverno carve-out** yet (`cluster/guardrails/`, `cluster/kyverno/`).
- **No game catalog** (`games/`).

See `docs/design/00-overview.md` for the build order and the open decisions (Agones packaging, node-pin vs. NodePort, NL front door, catalog format) that gate real implementation.

## Note

Before labeling the `game-servers` namespace `grizzly.io/gated=true`, the Agones SDK-sidecar image carve-out must exist (`cluster/kyverno/`) — otherwise Kyverno will reject the unsigned upstream sidecar. The namespace manifest in `grizzly-platform` ships without that label on purpose.
