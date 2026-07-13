# ADR-004 — S3-backed backups, archive, and restore

**Status:** Accepted (2026-07-07)

## Context

Per-instance game-server state — worlds, seeds, mods, `server.properties` — lives only on the instance's `iscsi-zfs` RWO PVC (`<instance>-data`, mounted at `/data`). It is deliberately not in git (ephemeral friend state, not infra; see the two config tiers in `00-overview.md`). Until now there was no durable, off-cluster copy of it: the supervisor's per-file `.grizzly.bak` sidecar is a config-rollback aid on the same PVC, not a backup, and `/destroy` deletes the PVC — and the world — with no recovery path.

We wanted three capabilities: periodic **automatic backups** of running servers, an **archive** that stops a server and releases its PVC while keeping a durable copy, and a **restore** that either rolls a server back to a backup or resurrects an archived one. Several design axes were open.

## Decision

**Bot-orchestrated S3 I/O.** The bot streams a tar of `/data` from the supervisor's control API straight to S3 (multipart), and the reverse for restore. S3 credentials live only in the bot (already gate-signed, already ESO-wired). Game pods — which run untrusted game-server software — never receive credentials and stay on the existing Cilium egress leash. The supervisor only gains streaming tar-out / tar-in routes (`GET`/`POST /archive`) plus a `SUPERVISOR_START_PAUSED` mode; it never touches S3. The alternative (supervisor uploads directly) would have put credentials in every game pod and opened S3 egress for all of them — a much wider blast radius, rejected.

**Object store: self-hosted versitygw `s3-bulk`** (`10.0.0.200:7072`, path-style, `us-east-1`) — the endpoint the platform S3 doc designates for backups (ADR-055 over in grizzly-platform). Because it is an RFC1918 address the egress leash blocks by default, the bot gets one additive `CiliumNetworkPolicy` carve-out (`cluster/guardrails/bot-to-s3-egress.yaml`) scoped to the bot component and TCP 7072 only; game pods stay leashed.

**S3 is the durable source of truth; Postgres is a rebuildable index — for archives only.** Every artifact writes a `manifest.json` sidecar next to its tarball, so the bucket is self-describing and any index can be rebuilt by scanning it (the pattern container registries, Velero, and Git-LFS all use). Automatic backups need no database: the live instance is their index, enumerated by an S3 prefix listing (`backups/<instance>/`). Archives destroy the instance, so they need a catalog to answer "what archives does this channel have?" — recorded in the foundation Postgres the bot already uses, treated as a fast, rebuildable projection of the manifests. Consequently archive / recover-from-archive require the DB and degrade gracefully without it (like the no-mention home channels), while backups and restore-from-backup keep working DB-less.

**S3 client: `rusty-s3` over the bot's existing reqwest.** `rusty-s3` only signs requests (SigV4 presigned URLs) that the bot's own reqwest client executes — it brings no second HTTP/TLS stack. The obvious alternatives were rejected on dependency grounds: `object_store` pulls a whole second reqwest (0.13) + quinn/QUIC + jni, and `aws-sdk-s3` pulls the aws-smithy runtime — either would have wrecked the crate's carefully curated TLS chain and the gate's `cargo deny`. Backups stream up as a multipart upload (one ~16 MiB part buffered at a time) and down as a body stream, so a multi-gigabyte world never fully buffers in the bot's read-only pod.

**No new container image and no new bot RBAC.** Backup logic lands in the already-gate-signed bot; the streaming tar routes land in the (gate-glob-excluded, per ADR-003) supervisor image. The bot already had `create,delete` on PVC/Service/GameServer and a supervisor egress path, so archive (delete) and recover (create) need no new verbs — keeping the gate surface minimal.

**Defaults:** automatic backups every **24h**, keep the last **7** per server (both overridable via env / `deploy/values.yaml`).

## Consequences

- Worlds now survive `/destroy` if archived first, and can be rolled back to any retained point. The archive area frees the PVC while keeping the world recoverable, so "I'm done with this for now" no longer means "delete it forever."
- Archive and restore-overwrite are destructive-adjacent, so both are gated behind an explicit Discord confirmation (slash-command buttons; Gary reuses the destroy-confirmation flow). Restore-overwrite additionally takes a safety backup of the current world first when possible, so the overwrite is normally undoable; when that safety backup can't be taken, restore still proceeds and tells the user there is no undo point rather than implying one exists.
- Recover-from-archive uses `SUPERVISOR_START_PAUSED`: the bot provisions the trio held down, seeds `/data` from the archive over the control API, then starts the game — so the game never generates a throwaway world that would be immediately overwritten.
- The DB coupling is scoped to archives only; a Postgres outage disables archive/recover but leaves backups and restore-from-backup fully working.
- New dependencies (`rusty-s3`, `jiff`, `tokio-util`, `tokio-stream`, `url`) all pass `cargo deny`. The supervisor image gains `zstd` for `tar --zstd`.
- Reversible: unsetting the S3 keys disables the whole feature (graceful "not configured" replies); removing the egress carve-out re-leashes the bot from S3.
