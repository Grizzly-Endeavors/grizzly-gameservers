# deploy — Helm chart (ADR-020 delivery contract)

`grizzly-platform`'s Flux `HelmRelease` for this app renders `./deploy` (see `kubernetes/apps/grizzly-gameservers/` over there). This is where the bot/agent workload, its ServiceAccount, and app-level wiring live.

`templates/deployment.yaml` is the bot/agent Deployment. `templates/externalsecrets.yaml` syncs the Discord bot token and Ollama Cloud key from OpenBao (`secret/grizzly-platform/gameservers/{discord,ollama}`) into namespace-local `discord-bot` / `ollama-api` Secrets via the `openbao` `ClusterSecretStore` — toggle with `externalSecrets.enabled` in `values.yaml`. The Deployment consumes those Secrets and never talks to OpenBao directly (the `game-servers` egress leash blocks `10.0.0.200:8200`).

Agones is packaged as a standalone gated `HelmRelease` rather than a Helm dependency of this chart (see [ADR-001](../docs/decisions/001-agones-packaging.md)); it and the `cluster/` guardrails + Kyverno carve-out are wired separately, not through this chart.
