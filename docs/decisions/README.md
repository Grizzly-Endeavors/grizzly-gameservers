# Decision Records

ADRs for grizzly-gameservers. Open decisions are tracked in `docs/design/00-overview.md` and become ADRs here as they resolve.

- [ADR-001 — Agones as a standalone gated HelmRelease](001-agones-packaging.md)
- [ADR-002 — NodePort routing, no node-pinning](002-nodeport-no-node-pin.md)
- [ADR-003 — Bot-scoped gate-signature enforcement](003-bot-scoped-gate-enforcement.md)
- [ADR-004 — S3-backed backups, archive, and restore](004-s3-backups-archive-restore.md)
- [ADR-005 — In-game chat triggers for the ops agent](005-ingame-agent-triggers.md)
- [ADR-006 — Guild-scoped tenancy and per-guild admin config](006-guild-tenancy.md)
- [ADR-007 — Deferred-task queue for the ops agent (`run_when`)](007-deferred-task-queue.md)
- [ADR-008 — Tool results are LLM-facing; only Gary's own replies adapt per surface](008-tool-result-formatting.md)
- [ADR-009 — Recover stranded instances by force-deleting the pod, not tainting the node](009-stranded-instance-recovery.md)
- [ADR-010 — Crash escalation suspends the health heartbeat instead of stopping it](010-recoverable-crash-escalation.md)
- [ADR-011 — An operator harness for driving the deployed bot](011-operator-harness.md)
- [ADR-012 — Per-instance environment lives in an overlay file on the PVC](012-per-instance-env-overlay.md)
- [ADR-013 — Build OTLP spans as `SpanData`, and take a second `reqwest` for the exporter](013-langfuse-span-construction.md)
- [ADR-014 — Record product events at the executing caller, in one wide table](014-product-event-log.md)
- [ADR-015 — A market-proxy cost plane: quantities in Postgres, money only at query time](015-market-proxy-cost-plane.md)
- [ADR-016 — Proactive watchdog: detection rides the sampler, response reuses the deferred-batch turn](016-proactive-watchdog.md)
- [ADR-017 — Bearer-token auth on the supervisor control API](017-supervisor-control-auth.md)
- [ADR-018 — Terraria runs the TShock flavor](018-terraria-tshock-flavor.md)
- [ADR-019 — The agent escalates by calling a tool, and that ends the turn](019-agent-invoked-escalation.md)
- [ADR-020 — Gary's conversation belongs to the channel, not to one person](020-channel-bound-conversations.md)
- [ADR-021 — Resize a running server in place; keep the override in Postgres](021-in-place-resource-resize.md)
- [ADR-022 — Per-guild resource caps, measured live and backstopped by a namespace quota](022-per-guild-resource-caps.md)
- [ADR-023 — Game pods are confined but not rootless](023-game-pod-partial-hardening.md)
- [ADR-024 — Backups are keyed by guild, and the existing ones were migrated](024-guild-scoped-backup-keys.md)

No open decisions currently tracked. The last one — the per-game catalog format — is resolved: generalized into `games/_template/` with a documented onboarding flow (see `docs/design/00-overview.md`).
