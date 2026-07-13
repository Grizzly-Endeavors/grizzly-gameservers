# Backups, archive & restore

Durable, off-cluster preservation of per-instance world data, plus the commands to move a server into and out of cold storage. This is the design of record; the decision rationale is [ADR-004](../decisions/004-s3-backups-archive-restore.md).

## What problem this solves

A server's world (seed, mods, `server.properties`, region data) lives only on its `iscsi-zfs` RWO PVC (`<instance>-data`, mounted at `/data`) — the *per-instance* config tier, deliberately not in git ([`00-overview.md`](00-overview.md)). Nothing preserved it off the PVC, and `/destroy` deleted it with the world. This adds:

- **Automatic backups** — periodic consistent snapshots of every running server (default every 24h, keep last 7), for point-in-time restore.
- **Archive** — stop a server, save a durable backup, and release the whole trio (PVC included). Cold storage for a server you're done with but might want back.
- **Restore** — roll a live server back to one of its backups, or recover an archived one (recreate the trio, reseed `/data`).

## Architecture: the bot orchestrates, the supervisor streams, S3 stores

```
Discord / Gary ── command ──▶ bot ──GET /archive──▶ supervisor ──tar /data──┐
                               │                                             │
                               │◀──────────── zstd tar stream ───────────────┘
                               │
                               └── multipart PUT ──▶ s3-bulk (10.0.0.200:7072)
                                                     backups/…  archives/…  + manifest.json
```

The **bot** is the only component that touches S3. It streams a `tar --zstd` of `/data` from the supervisor's control API and pipes it straight into an S3 multipart upload (one ~16 MiB part buffered at a time, so a multi-gigabyte world never fully buffers in the bot's read-only pod), and the reverse for restore. **S3 credentials never leave the bot** — game pods run untrusted server software and stay fully leashed. See ADR-004 for why this beats supervisor-side uploads.

### Supervisor surface (new)

Two streaming routes on the in-pod control API (`crates/control-api`, served by `crates/supervisor/src/control.rs`, tar logic in `archive.rs`):

- `GET /archive?quiesce=<bool>` — streams a zstd tar of the whole data root. `quiesce` flushes and pauses world saves first (Minecraft `save-off` + `save-all flush`, re-enabled when tar finishes) so a *live* backup is internally consistent; a no-op for games without RCON.
- `POST /archive?purge=<bool>` — extracts an uploaded tar stream into the data root. `purge` clears the root's contents first (overwrite-restore); confined to the data root, never the mount itself.

Plus `SUPERVISOR_START_PAUSED`: boot the control API but hold the game process down until `/start`, so the bot can seed `/data` before the first launch (recover-from-archive).

Unlike the per-file `fs` routes (kilobyte config edits), these stream — the small read/write caps deliberately do not apply. `tar` is used rather than a hand-rolled walker because it is battle-tested against the symlinks, permissions, and large files a real world tree carries.

## Storage layout

On the versitygw `s3-bulk` bucket `grizzly-gameservers` (path-style, `us-east-1`):

```
backups/<instance>/<stamp>.tar.zst        # automatic + manual; pruned to keep-N
backups/<instance>/<stamp>.manifest.json
archives/<guild_id>/<name>/<stamp>.tar.zst
archives/<guild_id>/<name>/<stamp>.manifest.json
```

`<stamp>` is a sortable UTC segment (`20260707T143000Z`), so a prefix listing is chronological without parsing. Each `manifest.json` is schema-versioned and self-describing (game, original instance, owning guild, created-by, timestamp, tarball key, size) — enough to recreate the instance without any database. That makes the bucket the **durable source of truth**; the Postgres archive index is a rebuildable projection of it.

## The two index strategies

- **Backups** are snapshots of a *live* instance, so the instance itself is the index: `list_backups` is an S3 prefix listing of `backups/<instance>/`. **No database needed** — this is why backups and restore-from-backup keep working even without Postgres.
- **Archives** destroy the instance, so there is no live object to hang them off. They are cataloged in the foundation Postgres (`archives` table, `crates/bot/src/backup/store.rs`) so `/archives` and `/recover` can answer "what does this guild have?". The catalog is a rebuildable cache of the manifests; **archive / recover-from-archive require the DB and degrade gracefully without it** (like no-mention home channels).

## Flows

All flows live in `crates/bot/src/backup/orchestrate.rs`, composing existing Agones lifecycle actions with the supervisor tar routes and the S3 shell (`s3.rs`).

- **Automatic backup cycle** — a Tokio interval task (spawned in `crate::run`) lists every live managed server and snapshots each to `backups/<instance>/`, quiescing running ones, then prunes to keep-N. Logs once per cycle, not per server.
- **`archive`** — ensure a pod is up (cold-start a shut-down server) → `supervisor_stop` (graceful world save) → stream `GET /archive` to `archives/<guild>/<name>/` + manifest → insert the Postgres row → `destroy_instance` (releases GameServer + Service + PVC). Nothing is released until the archive is durably in S3 **and** Postgres. If that final teardown step itself fails, the outcome is `ArchivedNotReleased`: the archive already stands (durable and `/recover`-able), just not freed yet — reported as safe to retry rather than as a failure.
- **`restore` (backup → live server)** — take a safety backup of the current world (best-effort: if there's no live target to snapshot or the snapshot itself fails, restore proceeds anyway) → `supervisor_stop` → download the chosen backup → `POST /archive?purge=true` → `supervisor_start` → wait for ready. When the safety backup couldn't be taken, the result says so explicitly — no undo point — instead of implying one exists.
- **`recover` (archive → new server)** — read the manifest/row → `provision_paused_instance` (trio held down via `SUPERVISOR_START_PAUSED`) → wait for the control API → download → `POST /archive` into the fresh PVC → `supervisor_start` → wait for ready → return the new `IP:port`. On failure the half-built server is torn down so the name frees up; the archive is untouched.

## Surfaces

Both a slash-command surface and Gary tools drive all four capabilities (`crates/bot/src/discord/commands.rs`, `discord/gary/tools.rs`):

| Slash command | Gary tool | Admin? | Confirm? |
|---|---|---|---|
| `/backup <server>` | `backup_server` | yes | no |
| `/backups <server>` | `list_backups` | no | — |
| `/archive <server>` | `archive_server` | yes | **yes** (releases the PVC) |
| `/archives` | `list_archives` | no | — |
| `/restore <server>` | `restore_server` | yes | **yes** (overwrites the world) |
| `/recover` | `recover_server` | yes | no (constructive) |

Everything is guild-scoped like the rest of the shim: a server or archive in another guild reads as "not found." A recovered archive is recreated in its **own** owning guild (read from the archive record), so a cross-guild operator recovering it doesn't re-home it. Destructive actions reuse the `/destroy` confirmation pattern; Gary's destructive tools post the same confirm buttons and require a human click even when the model requests them.

## Guardrails & delivery

- **Egress** — the bot gets one additive `CiliumNetworkPolicy` (`cluster/guardrails/bot-to-s3-egress.yaml`) opening the bot component → `10.0.0.200/32` TCP 7072 only. Game pods stay leashed; OpenBao (8200) and Postgres (5432) stay closed beyond their own carve-outs.
- **RBAC** — unchanged. Archive (delete) and recover (create) reuse the bot's existing PVC/Service/GameServer verbs; no Jobs, no VolumeSnapshot.
- **Images** — no new image. Backup logic ships in the gate-signed bot; the tar routes ship in the (gate-glob-excluded, ADR-003) supervisor image, which gains `zstd`.
- **Credentials** — `s3_access_key` / `s3_secret_key` are a foundation-store grant under OpenBao `stores/grizzly-gameservers`, provisioned by grizzly-platform's `setup-grizzly-gameservers-stores.yml --tags s3` and synced into the `game-servers` namespace by ESO. Absent keys leave the whole feature disabled (graceful "not configured" replies), the same shape as the DB/Ollama degradations.

## Open questions

- Gary's archive tool-result (`format_archive` in `crates/bot/src/discord/gary/tools.rs`) routes the `ArchivedNotReleased` partial-success outcome through the same `ArchiveDone` prompt as a clean archive, which overstates that the old server's storage was freed. The Discord slash-command embed already renders this outcome honestly (`archive_spec` in `crates/bot/src/discord/render.rs`); Gary's side needs a dedicated tool-result prompt for the partial-success case instead of reusing `ArchiveDone`. Deferred because it needs prompt-prose sign-off per the prompt-lib maintenance contract (`CLAUDE.md`).
