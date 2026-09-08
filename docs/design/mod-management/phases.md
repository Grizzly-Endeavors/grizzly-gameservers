# Minecraft Mods & Datapacks — Implementation Phases

> Module level only. No file or line references — those get mapped in each phase's own session. Each phase is self-contained, depends only on phases before it, and is verifiable on its own. Read `design.md` (same directory) first; it defines every contract referenced here.

## Phase 1 — Supervisor child-env overlay

- **Modules:** supervisor crate only — its config surface (two new env vars), a new pure module for parsing/filtering the overlay file, and the per-launch child-spawn path. No control-api changes, no bot changes.
- **Preconditions:** none.
- **Shape when done:** the supervisor implements the full overlay contract from `design.md` §"Per-instance child-env overlay": `SUPERVISOR_CHILD_ENV_FILE` / `SUPERVISOR_CHILD_ENV_ALLOWLIST` config, dotenv-subset parsing with the specified trim/duplicate/case/malformed-line semantics, allowlist filtering, the hard denies (`SUPERVISOR_` prefix, and the instance's *resolved* RCON password env name rather than a literal), the stated precedence order, idempotent seeding of the parent directory and an empty overlay file (at startup and in the per-launch preparation step, warn-and-continue on seeding failure), startup warnings for the two half-configured states, and per-key warning logs for skipped keys. The parsing/filtering logic is a pure module with unit tests covering every contract clause (missing file, malformed lines, duplicates, case mismatches, allowlist misses, hard denies, empty allowlist fail-closed). Unconfigured games behave byte-for-byte as before.
- **Verification:** `cargo test --quiet` green including the new unit tests; a `dev`-tagged Minecraft image push plus a scratch server confirms end-to-end mechanics — the seeded empty `.grizzly/env` appears on a fresh volume, a hand-authored allowlisted key visibly changes the child's behavior after an in-place restart, a non-allowlisted key is skipped with a warning in the supervisor logs, emptying the file returns the server to catalog defaults on the next restart, and the first overwrite of the seeded file reports that a snapshot was taken (the guardrail-eligibility property the design's seeding exists to guarantee).

## Phase 2 — Minecraft catalog enablement & empirical validation

- **Modules:** the Minecraft catalog entry (pod-template env and health timings) — plus a validation campaign on a scratch server, and whatever supervisor fix that campaign proves necessary.
- **Preconditions:** Phase 1 (overlay mechanism live in the Minecraft image).
- **Shape when done:** the Minecraft catalog declares `SUPERVISOR_CHILD_ENV_FILE=.grizzly/env` and the exact five-key allowlist from `design.md` §"Minecraft catalog enablement". The three upstream behaviors the design flags as verify-don't-assume are validated against the pinned itzg digest and the results recorded in this file under "Phase 2 findings": (1) the five env-var names match the pinned image's actual contract; (2) a first modded boot (`TYPE=FABRIC` plus at least one mod with a required dependency, plus one `datapack:`-prefixed datapack) completes within the bot-side verify ceiling — measure and note the actual boot time; (3) a `TYPE` switch on a volume that already has vanilla world state boots and loads the world (taking a backup first, per the workflow the design prescribes).
- **Verification:** a scratch Minecraft server reaches ready with Fabric, the mod, its auto-resolved dependency, and the datapack all present (confirmed via server logs and in-game); a deliberately bad slug in `MODRINTH_PROJECTS` produces the failure mode the guardrail loop needs (note whether itzg hard-fails the boot or warns-and-continues — this determines what the auto-rollback actually catches, and the observed behavior gets recorded in the findings); restoring the file's sibling snapshot and restarting returns the server to health; and an archive → restore cycle preserves the mod set (the overlay file rides the whole-volume archive) and boots back to ready still modded.

### Phase 2 findings

Recorded against itzg `java25-jdk@sha256:7e570b40…` on the `mod-test` scratch server.

**Upstream contract**

- **Env-var names confirmed.** `MODRINTH_PROJECTS` with the `datapack:` prefix resolves and installs (`Downloaded /data/world/datapacks/Terralith_26.2_v2.6.4.zip`, then Minecraft's `Found new data pack … loading it automatically`). The overlay reaches itzg exactly as designed.
- **Removal requires the key to stay present.** `MODRINTH_PROJECTS=` (empty value) triggers itzg's cleanup pass — `Removing old file world/datapacks/Terralith_26.2_v2.6.4.zip`. Deleting the line entirely is a **silent no-op**: itzg skips reconciliation when the var is unset, so the artifact stays installed while the declarative file claims otherwise. Phase 4's guidance must be "set the key to the remaining list, never delete the line."
- **A bad slug hard-fails the boot** (exit 2) rather than warning and continuing, so a typo is caught by the guardrail loop rather than by log inspection.

**First modded boot — 50 seconds, against a 5-minute ceiling**

A `TYPE=VANILLA→FABRIC` switch on a volume that already had world state, installing `chunky` (one required dependency) plus the `terralith` datapack, from `/restart` to the supervisor reporting `running`:

| | elapsed |
|---|---|
| overlay applied (4 keys) | +1s |
| `Resolving type given FABRIC` | +3s |
| Fabric launcher jar downloaded | +7s |
| `chunky` + auto-resolved `fabric-api` downloaded | +12s |
| Terralith datapack downloaded | +13s |
| Fabric Loader installed | +24s |
| datapacks found and loaded | +40s |
| `Done` — accepting connections | +48s |
| supervisor reports `running` | **+50s** |

Six times under the bot's `READY_WAIT_TIMEOUT`, so the verify loop reaches a verdict rather than degrading to inconclusive. No change to that ceiling is needed; Gary should still set the expectation that a first mod install takes a minute or so.

**`TYPE` switch on an existing world is safe.** The four overworld region files came through byte-identical in size, and the server logged `Preparing level "world"` rather than regenerating. Dependency resolution worked as documented — naming only `chunky` produced `Chunky-Fabric-1.5.3.jar` *and* `fabric-api-0.156.0+26.2.jar`.

**The guardrail now actually holds (issue #53, fixed).** A bad slug used to condemn the pod in ~25s against a minutes-scale verify ceiling, and repairing the file could not rescue it because the health heartbeat never resumed. Crash escalation now *suspends* the heartbeat and a relaunched child that reaches readiness resumes it — see ADR-010. Re-run with a nonexistent slug alongside a valid mod set: escalation at 55s, snapshot restored and restart issued immediately, `running` again 39s later, `game recovered after crash escalation; resuming agones health heartbeat`. The `GameServer` never left `Ready` and the pod's restart count stayed 0. Minecraft's `failureThreshold` widened 5 → 10 to give that repair a 150s window rather than 75s.

One case remains open by design: an overlay value that *nothing* rolls back is re-read by the recreated pod off the same PVC, so it crash-loops identically. Phase 3's `mod_info` is the pre-write validation that closes it.

**Archive → restore keeps the mod set.** A modded instance archived to S3 and recovered came back on a freshly provisioned PVC with `.grizzly/env` byte-identical (109 bytes), its `.grizzly.bak` sibling intact — so the snapshot lineage survives the round trip and the rollback guardrail is armed from the first edit after a recovery — and both jars plus the datapack in place. The world's four region files matched, and the recovered boot resolved the overlay (`keys=TYPE, VERSION, MODRINTH_PROJECTS, MODRINTH_DOWNLOAD_DEPENDENCIES`) and loaded Fabric. The purge-before-extract path re-seeds correctly: `child_env::seed` is `create_new`, so it treats the restored file as already present rather than truncating it.

**`VERSION` is now pinned.** The catalog previously set neither `TYPE` nor `VERSION`, so itzg resolved `LATEST` at every boot — meaning an upstream release would silently break a pinned mod set on the next restart. The catalog pins `VERSION: "26.2"` (the version instances were already running, so no world is downgraded), and instances raise it through the overlay. Confirmed live: `Resolved version given 26.2 into 26.2`.

## Phase 3 — Modrinth discovery tools in the bot

- **Modules:** bot crate — a new Modrinth client module (search + project lookup, typed responses, the design's timeout/User-Agent/truncation contract), Gary's tool registration/dispatch/formatting for the two new manager-tier tools, and the prompt-lib files for tool names, descriptions, parameters, and result text (with annotations, per the prompt-library maintenance contract). No supervisor or control-api changes.
- **Preconditions:** none strictly (independent of Phases 1–2); ordered here so live verification can exercise the whole discovery-to-install story.
- **Shape when done:** `search_mods` (query, optional project-type/loader/Minecraft-version filters) and `mod_info` (slug) exist at the manager tier per `design.md` §"Modrinth discovery": ≤5 results, 80/200-character title/description truncation, server/client-side flags rendered, one private formatter per tool per the single-format rule, API failures returned as plain explanatory text. The Modrinth endpoint shapes are re-verified against current upstream docs during implementation, per the design's caveat. Response parsing, facet construction, truncation, and formatting live in pure, unit-tested code; the annotations test passes.
- **Verification:** `cargo test --quiet` green including the new unit tests and the prompt-annotations test; after deploy through the normal bot CI, a manager-tier Discord conversation gets real, correctly-formatted results from `search_mods` and `mod_info`, everyone-tier does not see the tools, and an unknown slug to `mod_info` yields a friendly not-found result rather than an error.

### Phase 3 findings

Three assumptions this plan inherited from the design's API snapshot did not survive verification against the live endpoint, and the design's "External touchpoints" section now records the corrected contract: **`project_type` is a facet vocabulary and a response field with different value sets** (the facet accepts `datapack`, the field never returns it), a **search hit carries no `loaders` field** (they are folded into `categories` beside genuine tags, while a project lookup does return a clean array — hence two response shapes rather than one), and a hit's **`latest_version` is a base62 version id**, not the Minecraft version its documentation claims, so it is never rendered.

The `project_type` split cost a round trip and is worth stating plainly, because reading only the response field leads somewhere that looks right and isn't. Datapacks come back as `project_type: "mod"`, so the first implementation derived the datapack filter from that — `project_type:mod` AND `categories:datapack`. Every unit test passed, since they pinned the facet string rather than what it returns. The **deployed** check is what caught it: a datapack search for `terralith` returned one unrelated hit instead of 25, because the index holds a separate document per project type and the two terms select nearly disjoint sets. The fix is `project_type:datapack`. The lesson for a future phase is that a facet grammar is only verified by the results it returns, so a search filter needs at least one end-to-end assertion against the live registry — a pinned query string proves nothing about the query's meaning.

The phase also closed **issue #45** first: `search_mods` and `mod_info` would otherwise have had to be hand-registered in four places. The manager/admin partition now lives in one `tool_table()` of `(tier, spec)` pairs that the offer list, a single dispatch-time tier gate, and the below-tier refusal all read.

## Phase 4 — Gary workflow guidance & ship polish

- **Modules:** bot prompt files (new, additive guidance text — no rewrites of existing tuned prompt bodies), the decisions log (one ADR for the per-instance env overlay decision), and the platform overview's config-tier section (pointer to the new capability). Code changes only if Phase 2's findings demanded a verify-ceiling adjustment that hasn't landed yet.
- **Preconditions:** Phases 1–3 all live.
- **Shape when done:** Gary's manager-tier guidance covers the full workflow from `design.md` §"Gary workflow": discover → validate compatibility → warn on client-`required` mods → backup before first `TYPE` switch → never lower `VERSION` → author `.grizzly/env` with the existing file tools → restart and let the self-guarding loop verify → confirm from logs → set expectations that a first mod install takes a minute or so. Two rules the Phase 2 findings force are non-negotiable in that guidance: **removals set `MODRINTH_PROJECTS` to the remaining list and never delete the line** (an absent key is a silent no-op — itzg skips reconciliation and the artifact stays installed while the file claims otherwise), and **`VERSION` is written alongside the first `MODRINTH_PROJECTS`**, pinning the exact version the mods were validated against rather than relying on the catalog floor. The ADR records the overlay-on-PVC decision with the render-time alternative and its rejection reasoning. The design doc's status line is updated to reflect shipped reality.
- **Verification:** on the deployed bot, one end-to-end conversation per flow on a scratch server: a friend-style request installs a datapack via search → confirm → env edit → restart → verified healthy; a mod install on a vanilla server correctly triggers the backup-then-`TYPE`-switch path; a request for a client-`required` mod (e.g. a minimap) gets the plain-language "players must install this on their machines" warning; a deliberately broken install rolls back automatically and Gary reports it honestly. Docs merged; annotations test green.

### Phase 4 findings

The guidance shipped as two additive manager-tier blocks — `DiscordManagerModChoice` (what to install) and `DiscordManagerModInstall` (how) — appended unconditionally after `DiscordManagerTuning`. Everything the design's §"Gary workflow" prescribed was exercised on the `modflow` scratch server against the deployed bot; every flow passed on the first attempt, so nothing in this phase forced a change to the mechanism beneath it.

| flow | what Gary did |
|---|---|
| datapack ("more interesting terrain") | `search_mods` → `mod_info` → read `.grizzly/env` → wrote `TYPE=VANILLA`, `VERSION=26.2`, `MODRINTH_PROJECTS=datapack:terralith` → restart → `read_logs` confirmed the zip loaded |
| mod on a world with state | flagged the `TYPE` switch and the minute-or-so wait, asked first, then `backup_server` → `edit_file` → restart → healthy, `fabric-api` auto-resolved |
| client-`required` mod (minimap) | refused to install silently; explained in plain words that each player must install Fabric and the mod themselves, then asked whether to proceed |
| removal | rewrote `MODRINTH_PROJECTS` to the remaining slug rather than deleting the line; the jar was genuinely reconciled away (`mods/` empty afterwards) |
| bad slug | escalation → snapshot restored → healthy, reported honestly as a rollback rather than a success |

Two things are worth keeping in mind for later work. **The removal rule held, but nothing except the prompt makes it hold** — an absent key still reconciles nothing, so this is the phase's most fragile guarantee and the first thing to re-check if that block is ever reworded. And **`browse_files` does list `.grizzly/`**, so the reserved directory is genuinely discoverable rather than depending on Gary knowing the path from the prompt alone.

The bad-slug flow also re-confirmed [ADR-010](../../decisions/010-recoverable-crash-escalation.md) in place: the `GameServer` never left `Ready` and the pod's restart count stayed 0 through the crash-and-repair cycle.

The two blocks were the first game-specific prose in the system prompt, and every manager turn paid for them regardless of guild. That is closed: both moved verbatim behind a `game_guide` tool that loads one game's workflow on demand, and the system prompt keeps a single pointer at it. See [ADR-032](../../decisions/032-game-guidance-loads-on-demand.md).
