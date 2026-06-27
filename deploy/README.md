# deploy — Helm chart (ADR-020 delivery contract)

**Scaffolding — secret wiring only; no workload templates yet.**

`grizzly-platform`'s Flux `HelmRelease` for this app renders `./deploy` (see `kubernetes/apps/grizzly-gameservers/` over there). This is where the bot/agent workload, its ServiceAccount, and app-level wiring live.

`templates/externalsecrets.yaml` is the first real template: two `ExternalSecret`s that sync the Discord bot token and Ollama Cloud key from OpenBao (`secret/grizzly-platform/gameservers/{discord,ollama}`) into namespace-local `discord-bot` / `ollama-api` Secrets via the `openbao` `ClusterSecretStore`. Toggle with `externalSecrets.enabled` in `values.yaml`. The forthcoming bot/agent Deployment consumes those Secrets — it never talks to OpenBao directly (the `game-servers` egress leash blocks `10.0.0.200:8200`).

Open packaging decision (see `docs/design/00-overview.md`): whether Agones is pulled in as a Helm dependency of this chart or installed as a separate gated release, and how the `cluster/` guardrails + kyverno carve-out compose with the chart. Until that's resolved and real templates land, the Flux release for this app will sit NotReady — expected during scaffolding.
