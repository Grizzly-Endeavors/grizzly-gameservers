# ADR-006 — Guild-scoped tenancy and per-guild admin config

**Status:** Accepted (2026-07-08)

## Context

The tenant boundary was the Discord **channel** (introduced in commit `265efe3`, never backed by an ADR): every server was stamped with a `…/channel` label at create time, and every read/action confined itself to the caller's channel. A cross-channel super-admin (the user-id allowlist) was the sole exception.

That model conflated three unrelated "channel" ideas — Gary's home channel (`/gary-home`, no-mention answering), a server's owning tenant, and the channel a command happens to run in — and it broke in practice: `/archive` filed an archive under the server's owning-channel label while `/archives` and `/recover` looked archives up by the *current* channel and ignored scope. Archiving from one channel and recovering from another (same friend group) returned "nothing archived here." The channel-as-tenant key was a dev-era shortcut that got baked in.

## Decision

**Move the tenant boundary from the Discord channel to the Discord guild (server).** `CHANNEL_KEY` → `GUILD_KEY`, `ServerScope::Channel` → `Guild`, `channel_of` → `guild_of`. A server is stamped with the guild it was created in; all channels of a guild share one server pool and one archive namespace, which dissolves the archive/recover bug. One Gary still serves multiple friend groups — now each group is its own Discord guild, the natural isolation unit.

**Multi-guild by per-guild command registration.** The bot registers slash commands with each guild on the `GuildCreate` event (fires on startup for every guild and again on each join) — instant, no ~1h global-propagation wait, and no DM slash commands (intended). The single required `DISCORD_GUILD_ID` is dropped.

**Per-guild admin config in Postgres, set at runtime.** New `guild_admin_roles` / `guild_admin_users` tables and a `/config` command (`view`, `admin-role add|remove`, `admin-user add|remove`). Env `GAMESERVERS_ADMIN_USER_IDS` becomes the **cross-guild operator seed only**; `GAMESERVERS_ADMIN_ROLE_ID` is removed. Admin authority in a guild = operators ∪ guild **owner** ∪ DB-configured roles/users. The owner is the bootstrap path (a fresh guild is usable via `/config` with no env change). We chose **owner-based bootstrap over honoring raw Discord `Administrator` permission** — it's consistent across slash commands and Gary chat and avoids per-message permission computation; the owner covers the friends-scale case, and `Administrator`-permission bootstrap can be added later if needed.

**Fail-closed auth degradation.** `GuildConfig` mirrors the `HomeChannels` graceful-degrade pattern (in-memory cache, disabled when Postgres is down). With the config DB unavailable, `is_authorized` falls back to the implicit set only (operators + owner); nobody new is admitted, and `/config` mutations return "unavailable."

**DM behavior.** A DM has no guild. A non-operator asking Gary to act in a DM is refused with guidance to use a guild channel. A cross-guild **operator** keeps the all-guilds view in a DM and manages any server there via Gary (slash commands don't exist in DMs under per-guild registration).

**Archives follow the guild.** S3 prefix `archives/<guild>/<name>/`; the Postgres `archives` table keys on `guild_id`; `/archives` and `/recover` scope by the caller's `ServerScope` (a guild list, or an all-guilds list for an operator). Recover stamps the recreated server with the **archive's own owning guild** (from the record), fixing a latent mis-stamp where a cross-guild recovery would re-home the server.

## Consequences

- Servers and archives are shared across a whole Discord server; the channel a command runs in no longer matters, which is what non-technical friends expect. Home channels (`/gary-home`) are now clearly *only* about no-mention answering.
- Friend-group isolation is now at the guild level (separate Discord servers) rather than channels within one guild. Groups sharing a single guild would share servers — acceptable and simpler at friends-scale.
- The bot is genuinely multi-guild: one deployment/token can serve many guilds, each with its own admins and archives.
- **Migration is a wipe** (disposable dev data): existing `CHANNEL_KEY` servers are deleted and the `home_channels` / `archives` tables dropped so they recreate guild-keyed on next start; old `archives/<channel>/…` S3 objects are abandoned (unreferenced). Detailed in the change's runbook.
- This **supersedes the channel-tenancy model from commit `265efe3`** (which had no ADR).
