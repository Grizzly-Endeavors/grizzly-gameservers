# Thunderstore Mods (Valheim first, r2modman-shaped) — Design

**Status:** Approved, not yet implemented. This is the design of record for mod management on every game whose modding ecosystem is Thunderstore/r2modman. The phases and their verification checklists are in `phases.md` (same directory). The Minecraft/Modrinth design it extends is [`../mod-management/design.md`](../mod-management/design.md) and [ADR-012](../../decisions/012-per-instance-env-overlay.md).

> This document must stand on its own: it will be implemented in fresh sessions that have only this doc, `phases.md`, and the codebase — not the conversation that produced it. Every external fact it relies on is stated here with how it was verified, because most of them came from reading upstream source and live API responses rather than documentation.

## Goal & context

Friends should be able to ask Gary for mods on a Valheim server — "add a better map", "install Jotunn" — and get the same treatment Minecraft gets today: discovery grounded in the live registry, a declarative install that is snapshotted and rolled back automatically if it breaks the server, and plain-language warnings when a mod has to be installed on the players' own machines too.

The design is deliberately **not** a Valheim adapter. Valheim is the first game on it, but the pattern is for the whole family of games that r2modman/Thunderstore Mod Manager serves (Palworld with UE4SS, Lethal Company, Risk of Rain 2, …): a Thunderstore community per game, a loader the image provides, and a per-game rule for where package files go. The supervisor half knows nothing about Valheim; the Valheim-specific knowledge is three environment variables in its Dockerfile, one line in its catalog manifest, and one shell hook in its image.

### Why the Minecraft approach does not transfer

Minecraft mod management installs nothing itself: the itzg image re-resolves a `MODRINTH_PROJECTS` env var at every start, so the platform only needed a per-instance env tier (ADR-012). lloesche's `valheim-server` image, which the Valheim catalog entry wraps, has no equivalent. Verified by reading its `common`, `valheim-updater`, `bepinex-updater`, and `valheim-server` scripts on the `main` branch:

- With `BEPINEX=true`, `bepinex-updater` fetches `denikson/BepInExPack_Valheim` from the Thunderstore experimental API and `merge_mod` rsyncs the Valheim server files plus the pack into `/opt/valheim/bepinex/` — an image-layer path that survives in-place restarts inside a pod but is rebuilt on pod recreation.
- `merge_mod` rsyncs `/config/bepinex/plugins/` (PVC) into the staged `BepInEx/plugins` **only at merge time** — a BepInEx pack update or a Valheim update — and without `--delete`. An in-place restart never syncs it, and a removed plugin is never removed.
- `BepInEx/config` inside the install is deleted and symlinked to `/config/bepinex` (PVC), so per-plugin `.cfg` files persist and are reachable by Gary's file tools under the data root.
- `PRE_SERVER_RUN_HOOK` runs after BepInEx setup and before every server start, blocking. The Valheim image already uses it (`valheim-seed-rcon.sh`) to re-place the baked ValheimRcon DLL into the live plugins dir on every boot, precisely because that dir is not persistent.
- The launch sets `DOORSTOP_ENABLED=1`, `DOORSTOP_TARGET_ASSEMBLY=./BepInEx/core/BepInEx.Preloader.dll`, `LD_LIBRARY_PATH` to the pack's `doorstop_libs`, and `LD_PRELOAD=libdoorstop_x64.so`. The reconciler never touches any of this; the loader stays lloesche's.
- Game pods already have egress to thunderstore.io (lloesche's updater uses it), so no NetworkPolicy change is needed.

So the platform has to own package resolution for this family, and the image has to mirror the result into the live loader directory before each start.

### Verified Thunderstore facts

All fetched live on 2026-09-03 against the Valheim community.

**Search** — `GET https://thunderstore.io/api/cyberstorm/listing/{community}/?q=<text>&ordering=most-downloaded&page_size=N` (optionally `&included_categories=<id>`) returns:

```json
{"count": 153, "next": null, "previous": null,
 "results": [{"namespace": "Mydayyy", "name": "ServerSideMap",
   "description": "…", "download_count": 114094, "size": 796116,
   "categories": [{"id": "17", "name": "Mods", "slug": "mods"},
                  {"id": "29", "name": "Server-side", "slug": "server-side"},
                  {"id": "30", "name": "Client-side", "slug": "client-side"}],
   "is_deprecated": false, "is_nsfw": false, "is_pinned": false,
   "last_updated": "2025-03-12T00:31:53Z", "icon_url": "…", "rating_count": 301}]}
```

Category ids are per community (`GET /api/cyberstorm/community/{community}/filters/` lists them), so filter on slugs client-side rather than sending ids. Valheim's slugs include `mods`, `modpacks`, `libraries`, `tools`, `server-side`, `client-side`, `utility`, `world-generation`, plus one slug per game update ("bog-witch-update"). The community endpoint (`/api/cyberstorm/community/valheim/`) reports 11,159 packages; there is no need to enumerate them.

**Package** — `GET https://thunderstore.io/api/experimental/package/{namespace}/{name}/` returns:

```json
{"namespace": "denikson", "name": "BepInExPack_Valheim",
 "full_name": "denikson-BepInExPack_Valheim", "is_deprecated": false,
 "date_updated": "2025-08-29T13:16:39Z", "is_pinned": true,
 "latest": {"version_number": "5.4.2333",
   "full_name": "denikson-BepInExPack_Valheim-5.4.2333",
   "description": "…", "dependencies": [],
   "download_url": "https://thunderstore.io/package/download/denikson/BepInExPack_Valheim/5.4.2333/",
   "downloads": 1208923, "date_created": "2025-08-29T13:16:39Z",
   "website_url": "https://github.com/AzumattDev/BepInEx", "is_active": true},
 "community_listings": [{"community": "valheim", "has_nsfw_content": false,
   "categories": ["Hildir's Request Update", "Ashlands Update", "Bog Witch Update"],
   "review_status": "unreviewed"}]}
```

`GET …/{namespace}/{name}/{version}/` returns one version in the `latest` shape (verified with `ValheimModding/Jotunn/2.28.0`, whose `dependencies` is `["denikson-BepInExPack_Valheim-5.4.2333"]`). Dependency strings are always `Namespace-Name-Major.Minor.Patch`. An unknown package returns 404. The download URL is `https://thunderstore.io/package/download/{namespace}/{name}/{version}/` and serves a zip. Reads need no authentication; Thunderstore publishes no rate limit, so the client backs off rather than assuming one. **`GET /c/{community}/api/v1/package/` is 162 MB for Valheim and must never be used.**

**Package zips** vary in layout. Verified examples:

- `ValheimModding-Jotunn-2.28.0`: `manifest.json`, `README.md`, `CHANGELOG.md`, `icon.png`, `plugins/Jotunn.dll` (+ `.pdb`, `.xml`, `.mdb`).
- `Smoothbrain-Sailing-1.1.8`: `manifest.json`, `README.md`, `icon.png`, `Sailing.dll` at the root.
- `shudnal-ConditionalConfigSync-1.0.0`: root-level DLLs plus `SHA256SUMS.txt`.
- `denikson-BepInExPack_Valheim-5.4.2333`: `BepInExPack_Valheim/BepInEx/{core,config,patchers,plugins}/…`, `doorstop_config.ini`, `.doorstop_version` — the loader pack, which the image provides and the reconciler must skip.

`manifest.json` inside every zip carries `name`, `version_number`, `description`, `website_url`, `dependencies` (the same dependency strings), so a cached zip is enough to resolve a pinned entry without the network.

**r2modman's BepInEx placement rule** (from Thunderstore's packaging guide and r2modman's install rules; verify the per-package-subfolder detail against r2modman's source when implementing and pin it with fixture tests): a top-level `plugins`, `patchers`, `monomod`, or `core` directory — with or without a leading `BepInEx/` — maps to `BepInEx/<that dir>/<Namespace-Name>/…`; `config` maps flat to `BepInEx/config/…`; everything else (bare DLLs, asset folders) goes under `BepInEx/plugins/<Namespace-Name>/…`; root `manifest.json`, `README.md`, `CHANGELOG.md`, `icon.png` are not installed.

**Side semantics.** Thunderstore has no server/client flag; the Valheim community expresses it with categories. `server-side` and `client-side` together means the mod must be installed on the server and on every player's machine; `server-side` alone means server-only; `client-side` alone means there is nothing to install server-side; neither means unknown, and the friend should be pointed at the mod page. Mod-list enforcement toward clients (ServerSync, Jotunn's version check, ValheimEnforcer) is itself mod-provided and out of scope here.

**Timing.** Valheim leaves early access in September 2026. Unity engine bumps have broken the BepInEx pack before (patch 0.217.46). The pack is lloesche's updater's concern; this design's only coupling to it is the "provided" package name, so a pack break shows up as the loader failing to load in `read_logs`, not as a reconciler fault.

### Verified facts about this codebase that shape the failure path

- The bot's `wait_for_ready` polls the supervisor's `/status` and returns `Crashed` on `ProcessPhase::Crashed`; that return is what arms its snapshot-restore-and-restart after a config change. It treats an unreachable control API as transient and keeps waiting within its five-minute ceiling.
- The bot's restart tool runs that verify loop only when the supervisor replied `Restarted`; a `Failed` reply skips it. A launch-preparation failure therefore has to reply "restarted" and let `/status` say `Crashed`.
- Gary's `read_logs` reads the supervisor's child-output `LogBuffer` (its `push` is public), not the supervisor's own tracing output. Anything Gary must see goes into that buffer.
- The supervisor's initial spawn in `runner::supervise` propagates errors with `?`, which would exit PID 1; a preparation failure there must instead leave the control API up so the file can be repaired.
- `child_env::seed` creates a file with `create_new` so a per-launch re-seed can never truncate an authored file; the bot's `write_file` only snapshots a file that already exists, and the auto-rollback only arms for writes that took a snapshot. Seeding empty is what makes the very first edit rollback-eligible.
- The catalog (`crates/bot/src/agones/catalog.rs`) holds each game's three templates as raw strings with no structured metadata; the only precedent for per-game metadata is Service annotations parsed in `crates/bot/src/agones/ports.rs`.

## Decisions (made by the user, binding)

1. **Resolution lives in the supervisor, pre-launch.** A generic Rust reconciler runs in the same per-launch preparation chain as the env overlay. No new control-API routes, no binary-write surface for Gary, and the install inherits snapshot → apply → verify → auto-rollback unchanged.
2. **The mod set is a dedicated file, `.grizzly/mods`.** Not a key in `.grizzly/env`: the supervisor should not be reading its own instructions out of the child's environment, and the overlay's hard-deny on `SUPERVISOR_` keys makes the naming awkward.
3. **One discovery tool pair keyed by game.** `search_mods` / `mod_info` take a required `game` and the catalog declares each game's registry. Fewer tools per turn and one workflow for Gary.
4. **Game guidance loads on demand, skill-style (closes issue #61).** A `game_guide` tool returns the workflow for a game; the system prompt keeps only a pointer.

Rejected: the bot downloading packages and pushing them through a new binary-write route (ADR-012 already rejected it: an LLM loop doing dependency resolution, and installed state outside the rollback loop); a shell hook in the image doing the fetch (untestable, and every r2modman game would copy-paste it); gating guidance by the guild's live game set (needs plumbing the system prompt builder does not have, and still pays on every turn in a guild with the game).

## Shape

### 1. Catalog: the registry a game's mods come from (bot)

Annotations on `gameserver.yaml` `metadata.annotations`:

- `grizzly-gameservers.grizzly-endeavors.com/mod-registry: modrinth | thunderstore`
- `grizzly-gameservers.grizzly-endeavors.com/thunderstore-community: <slug>` (required iff the registry is `thunderstore`)

A pure parser is strict (unknown registry, thunderstore without community, community without thunderstore → error); the catalog loader wraps it leniently (warn and "no registry") so a malformed annotation cannot stop the bot from starting. The catalog entry gains `mod_registry: Option<ModRegistry>`. Minecraft declares `modrinth`; Valheim declares `thunderstore` + `valheim`; every other game declares nothing and its `search_mods` calls get a plain "this game takes no mods here" result.

### 2. Supervisor: generic Thunderstore reconciler

**Configuration** (image facts in the Dockerfile `ENV`, policy in `gameserver.yaml`, per the template README's rule for the env overlay):

| Variable | Where | Meaning |
|---|---|---|
| `SUPERVISOR_MODS_FILE=.grizzly/mods` | `gameserver.yaml` | data-root-relative; presence enables the reconciler |
| `SUPERVISOR_THUNDERSTORE_COMMUNITY=valheim` | Dockerfile | which community `search_mods` and the resolver talk to |
| `SUPERVISOR_THUNDERSTORE_LAYOUT=bepinex` | Dockerfile | closed enum, one variant today; an unknown value is a hard error |
| `SUPERVISOR_THUNDERSTORE_PROVIDED=denikson-BepInExPack_Valheim` | Dockerfile | comma list of packages the image supplies, matched by `Namespace-Name` |
| `SUPERVISOR_THUNDERSTORE_DIR` | optional, default `.grizzly/thunderstore` | holds `cache/`, `staged/`, `resolved` |

The reconciler is assembled only when both the file and the community are set; either alone logs a startup warning (same precedent as the half-configured env overlay). Both paths must resolve inside the data root.

**The mods file.** One package per line: `Namespace-Name` (latest at resolve time) or `Namespace-Name-1.2.3` (pinned). `#` comments and blank lines are ignored. Components are `[A-Za-z0-9_]+`; versions are three numeric parts. A malformed line or a package listed twice is an error that names the line — unlike the env overlay, a typo here must fail loudly so it is rolled back. Seeded empty at supervisor startup and before every launch with the same `create_new` seeding the env overlay uses (hoisted into a shared helper).

**Per launch:**

1. Parse the file.
2. Resolve transitively through a `PackageSource` trait (API first, on-disk cache as fallback): explicit pins are hard; a dependency's version is a floor; the highest version wins per package; provided packages are skipped by name; cycle-safe; bounded (256 packages, depth 32). A dependency floor above an explicit pin honours the pin and warns, because Thunderstore manifests declare "built against", not "requires".
3. Download missing zips to `cache/<Namespace-Name-1.2.3>.zip.part` and rename atomically. Per-zip cap 256 MiB (content-length and streamed bytes), entry-count cap, total-uncompressed cap.
4. Validate every zip: reject `..`, absolute or backslash-rooted paths, NUL bytes, symlink entries; the inner `manifest.json` name and version must match the filename.
5. Rebuild `staged/` from scratch into `staged.next`, then swap. The staged tree is the loader-relative layout (`BepInEx/plugins/<Namespace-Name>/…`, `BepInEx/patchers/…`, `BepInEx/config/…`).
6. Write `resolved`: one `Namespace-Name-1.2.3` per line, with a comment naming provided packages that were skipped. Plain data Gary can `read_file`.
7. Delete cache zips outside the resolved set.
8. Push `[mods] installed …` / `[mods] failed: …` lines into the child-output log buffer so `read_logs` shows them. Log package names, never file contents.

**Offline behaviour.** Pinned entries resolve entirely from cached zip manifests, so a restart with an unchanged pinned list needs no network. Unpinned entries fall back to the highest cached version with a warning when the API is unreachable. Nothing cached and no network is a failure.

**Failure signalling.** A preparation failure (malformed line, unknown package or version, download failure with nothing cached, rejected zip) fails the launch and is reported as a crash: the state machine gains `on_launch_failed()` (phase `Crashed`, restart counter unchanged), the runner logs the reason and pushes it into the log buffer, and `/start` and `/restart` still reply "acted" so the bot's verify loop runs, sees `Crashed`, restores the file's snapshot, and restarts. The initial spawn takes the same recoverable path so a pod recreated on a PVC with a bad file keeps a reachable control API instead of crash-looping the container. Spawn failures proper stay fatal as today. The reconcile runs inline in the supervise loop under a 180-second deadline; if live verification shows control-request starvation during long downloads, the follow-up is to run it in a spawned task with its own select arm.

**Layout rule** (`plan_layout`, pure): strip an optional leading `BepInEx/`; first path component (case-insensitive) `plugins`, `patchers`, or `monomod` → `BepInEx/<dir>/<Namespace-Name>/<rest>`; `config` → `BepInEx/config/<rest>`; `core` → skipped with a warning (a non-provided package must never overwrite the image's loader); anything else → `BepInEx/plugins/<Namespace-Name>/<path>`; root `manifest.json`, `README.md`, `CHANGELOG.md`, `icon.png` skipped. Adding a UE4SS layout later is one enum arm plus fixture tests.

### 3. Valheim image: mirror the staged tree

The pre-server hook (today `valheim-seed-rcon.sh`, renamed to reflect its wider job) gains one step before the RCON steps: if `<data>/.grizzly/thunderstore/staged/BepInEx` exists, `rsync -a --delete --exclude ValheimRcon.dll` its `plugins/`, `patchers/`, and `monomod/` into `/opt/valheim/bepinex/BepInEx/`, and `rsync -a --ignore-existing` its `config/` into `/config/bepinex/` — never overwriting a config file a friend or Gary has edited, and never touching `core/`. When the staged tree is absent the hook does nothing new, so an image with the reconciler unset behaves as it does today. The Dockerfile gains the three `SUPERVISOR_THUNDERSTORE_*` variables; the catalog manifest gains `SUPERVISOR_MODS_FILE`, the registry annotations, and a health `failureThreshold` of 10 (Minecraft's reasoning: a rollback re-resolves and may re-download, and the repair must land inside the Agones health window).

### 4. Bot: discovery keyed by game

- `search_mods` and `mod_info` take a required `game` (catalog id). `search_mods` gains an optional `side` (`server`, `client`, `both`). `kind`, `loader`, and `game_version` remain and become Minecraft-only, reported as ignored for other registries rather than silently dropped.
- A `thunderstore` module mirrors the `modrinth` one: `search_packages(http, community, query)` (page size 20, deprecated and NSFW listings dropped, side filter on category slugs, truncated to five hits) and `fetch_package(http, namespace, name)` (404 → not found). Ten-second timeout, identifying User-Agent, `#[ignore]` live tests against the real API — the Modrinth phase showed a facet grammar is only verified by what it returns.
- Truncation helpers and limits (80-character titles, 200-character descriptions) move to a shared `registry` module with a `Side` filter and a `SideSupport { ServerOnly, ClientOnly, Both, Unknown }` classification. Modrinth expresses `side` through its `server_side`/`client_side` facets.
- Results for Thunderstore packages render `Namespace-Name`, the current version string (what Gary pins), downloads, side support, a deprecation flag, and up to eight dependencies so Gary can see the loader pack is among them and provided.
- The existing `SearchMods` and `ModInfo` prompt files say "Search Modrinth for Minecraft mods" and "Modrinth slug"; adding `game` makes that text wrong, so their bodies and parameter descriptions are rewritten registry-neutral (the `slug` parameter described as "a Modrinth slug or a Thunderstore Namespace-Name"), with the Minecraft-only filters labelled as such and both descriptions pointing at `game_guide`. This rewrite was approved with the plan.

### 5. Bot: `game_guide`

A manager-tier tool that targets no server: `game_guide(game)` returns the mod workflow for that game's registry, composed from prompt files (an intro naming the game, then the registry's "what to install" and "how to install" blocks; or a "this game takes no mods here" or "unknown game, here is the catalog" result). The two Minecraft blocks that are appended to every manager turn today move verbatim into the Modrinth guide. A new Thunderstore guide pair carries: identify packages by `Namespace-Name`, never from memory; `search_mods` for this game then `mod_info` to confirm the name, current version, dependencies, and side; the side semantics above and the warn-and-ask for both-sided mods; install is one line per package in `.grizzly/mods`, **pinned** to the version `mod_info` reported; dependencies come automatically and the provided loader pack is never listed; **removal is deleting the line** (the opposite of the Minecraft rule, because the installed set is rebuilt from the file on every start); the restart applies it and self-guards; confirm via the `[mods]` log lines and `.grizzly/thunderstore/resolved`; mod config files appear under the data directory after the first boot and are edited like any config file; back up before the first install on a world with state; a bad line stops the server and is rolled back; a first install takes a minute. The system prompt's manager guidance drops the two Minecraft blocks and gains one short game-agnostic pointer: before searching for, recommending, installing, or removing a mod on a server, call `game_guide` for that game and follow it.

## Integration with existing system

- **Supervisor launch chain.** Order per launch: Palworld RCON seed → Thunderstore prepare → env overlay prepare → spawn. Every path that launches (first boot, `/start`, `/restart`, crash relaunch, update-on-empty relaunch, archive recovery) passes through it.
- **Snapshot → apply → verify → auto-rollback.** Untouched and load-bearing, as in ADR-012: editing `.grizzly/mods` through the file tools produces the standard sibling snapshot, and the bot's verify loop restores it when the next launch reports `Crashed`. This design adds no rollback code.
- **Config tiers.** The per-game tier now also declares a game's registry and (for Thunderstore games) its community, layout, and provided packages; the per-instance tier gains `.grizzly/mods` and the `.grizzly/thunderstore/` working tree beside `.grizzly/env`. Both ride the whole-volume archive, so a recovered server resolves offline from its own cache.
- **Watchdog.** `Crashed` is a crash signal, so an unrepaired reconcile failure raises an incident. Intended.
- **Drift guards.** New: a game with `SUPERVISOR_MODS_FILE` in its manifest must have `SUPERVISOR_THUNDERSTORE_COMMUNITY` in its Dockerfile and a matching `thunderstore` annotation, and a `modrinth` game must declare `SUPERVISOR_CHILD_ENV_FILE`. Existing guards (graceful-stop window, security baseline, control port, readiness-port agreement — Valheim exempt via its log pattern) are unaffected by env and annotation additions.
- **Untrusted text.** Package names and descriptions enter Gary's context under the same posture as Modrinth results and game logs; issue #81 applies equally.

## Open questions

None the phases do not close empirically: the per-package-subfolder detail of the layout rule, file ownership of root-written DLLs against the `valheim` runtime user, first-install wall time against the bot's readiness ceiling, and whether the inline reconcile starves control requests are all verified on a scratch server in Phase 3 and recorded in `phases.md`.
