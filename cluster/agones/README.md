# cluster/agones

Agones operator standup, gated here on purpose — the gate vets external repos before deploy, so security-sensitive standup belongs where the gate sees it, not in `grizzly-platform`. See `docs/design/00-overview.md` → "Gate + Flux integration".

- `helmrepository.yaml` / `helmrelease.yaml` — Agones (pinned `1.58.0`) installed into `agones-system` as its own Flux `HelmRelease`, independent of the app `deploy/` chart. CRDs install with the chart (`crds: CreateReplace`), so a cold sync applies CRDs before any GameServer CR. The `./games` Flux Kustomization `dependsOn` this one for CRD-before-CR ordering.
- Watches the `game-servers` namespace (`gameservers.namespaces`); that namespace ships in `cluster/guardrails` and must exist before this release provisions its per-namespace SDK RBAC — both render under the same `./cluster` Kustomization.
- Agones' own dynamic hostPort range is pinned to `8000–8100`, clear of the `7000–7010` NodePort band the edge forwards. Game servers are exposed via NodePort Services (see `games/`), not hostPort, so this range should stay unused; the offset just prevents an accidental `Dynamic` GameServer from grabbing an edge port.
