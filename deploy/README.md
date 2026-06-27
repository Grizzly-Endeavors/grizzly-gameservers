# deploy — Helm chart (ADR-020 delivery contract)

**Placeholder — no real templates yet.**

`grizzly-platform`'s Flux `HelmRelease` for this app renders `./deploy` (see `kubernetes/apps/grizzly-gameservers/` over there). This is where the bot/agent workload, its ServiceAccount, and app-level wiring live.

Open packaging decision (see `docs/design/00-overview.md`): whether Agones is pulled in as a Helm dependency of this chart or installed as a separate gated release, and how the `cluster/` guardrails + kyverno carve-out compose with the chart. Until that's resolved and real templates land, the Flux release for this app will sit NotReady — expected during scaffolding.
