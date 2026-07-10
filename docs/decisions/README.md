# Decision Records

ADRs for grizzly-gameservers. Open decisions are tracked in `docs/design/00-overview.md` and become ADRs here as they resolve.

- [ADR-001 — Agones as a standalone gated HelmRelease](001-agones-packaging.md)
- [ADR-002 — NodePort routing, no node-pinning](002-nodeport-no-node-pin.md)
- [ADR-003 — Bot-scoped gate-signature enforcement](003-bot-scoped-gate-enforcement.md)
- [ADR-004 — S3-backed backups, archive, and restore](004-s3-backups-archive-restore.md)
- [ADR-005 — In-game chat triggers for the ops agent](005-ingame-agent-triggers.md)
- [ADR-006 — Guild-scoped tenancy and per-guild admin config](006-guild-tenancy.md)

No open decisions currently tracked. The last one — the per-game catalog format — is resolved: generalized into `games/_template/` with a documented onboarding flow (see `docs/design/00-overview.md`).
