# CLAUDE.md | grizzly-gameservers

This file provides guidance to Claude Code when working with code in this repository.

## Project Overview

**Gary — an LLM ops agent that operates live production systems unattended — and the game-server platform that gives him something real to run.** Gary is the product; `grizzly-gameservers` is the project that houses him ([ADR-028](docs/decisions/028-gary-is-the-product.md)). The names differ on purpose: every identifier here — repo, crates, namespace, image, DB role, secret paths — says `grizzly-gameservers`, and the prose says Gary. Take the prose.

The domain is game servers because it is an unusually good proving ground: every game stores its config somewhere different, the users are non-technical friends with real worlds at stake, and a mistake costs a game world rather than a payments ledger. Servers run as containers in the homelab Kubernetes cluster (via **Agones**); the Hetzner proxy VPS is the public edge. A friend asks Gary for something in Discord — in plain English, or through a slash command when they'd rather not — and he does it.

**The test any change has to pass is whether it makes Gary a more capable or safer operator.** "Does it improve game hosting" is a proxy that sometimes points the other way, most sharply on anything that would buy hosting efficiency by widening what the agent may do unsupervised.

**Gary is the only reader of a server's filesystem.** The per-instance volume — the game's own config, `.grizzly/env`, `.grizzly/mods`, the reconciler's working tree, everything under the data root — has exactly one audience, and it is the agent. Friends never see it, and bear has no easy access to it either; absorbing the need for anyone to look at that stuff is the point of the product, not an accident of the interface. So "a friend reading this file would be confused" is not a real cost and must never be weighed as one — nobody is going to be confused, because nobody is going to read it. The reason per-instance files are still kept clean, ordered, and self-describing is narrower and worth stating: it makes **Gary** more reliable. A format he can parse unambiguously, an artifact he can read back to confirm what a change actually did, a file whose meaning doesn't depend on remembering a convention. Optimize per-instance file layout for the agent; don't invent a human audience for it.

**Read `docs/design/00-overview.md` first** — it's the architecture of record. The short version:

- **Ops agent ("Gary")** — the product. An LLM loop that operates running servers: read config + logs, decide the change, mutate the right file, restart, verify, roll back if it didn't take. It exists to absorb each game's idiosyncratic config layout *generically* instead of hand-rolling a per-game adapter for every game. He works through the per-pod supervisor's typed HTTP control API, never `kubectl exec` — the typed contract is what makes the guardrails enforceable. One of those routes is a **read-only shell** (`run_shell`): real `sh -c` commands, because no bespoke search route competes with a model's fluency at `grep`, made read-only by a Landlock ruleset rather than by curating the verbs — so config changes still flow through the typed writes that snapshot, and `/proc` stays unreadable because it carries the control token ([ADR-034](docs/decisions/034-agent-read-only-shell.md)). `read_file` returns a window of numbered lines and says how many lines the file has, so a large config is grepped and then read narrowly rather than pulled in whole; `edit_file` replaces its anchor inside the pod, so what can be edited doesn't depend on what fits in a read ([ADR-036](docs/decisions/036-context-is-sized-against-the-window.md)). The thing worth searching is preserved rather than lost: each launch's output is captured on its own and written to the instance's volume under `.grizzly/crashes/` when that launch dies, because the auto-rollback's restart would otherwise overwrite the only account of what it rolled back ([ADR-035](docs/decisions/035-preserve-a-crashed-launch.md)). Those guardrails are the core claim, not overhead on it: namespace-scoped blast radius, snapshot→apply→verify→auto-rollback, and an escalation exit — an `escalate` tool Gary can call at any tier to hand a problem to a human, which ends the turn on the spot; the bounded round budget is the backstop behind it ([ADR-019](docs/decisions/019-agent-invoked-escalation.md)).
- **Per-pod supervisor** — Gary's hands. A Rust binary baked into each game image as PID 1, wrapping the game as its child, serving the file/lifecycle/console control API he drives. Because it owns the child process, a restart is seconds in place rather than a pod reschedule — which is what makes the verify-and-roll-back window short enough to be real. It is also the seam that keeps the substrate replaceable: a second backend behind this contract is a connector, not a rewrite.
- **Discord shim** — the surface Gary is reached through, deliberately thin: auth the friend, map a slash command to an Agones allocation/teardown, return `IP:port`. No billing; generous per-guild caps on server count and aggregate CPU/memory bound the blast radius since the stranger beta ([ADR-022](docs/decisions/022-per-guild-resource-caps.md)). Behind those caps the namespace `ResourceQuota` is the hardware ceiling, and the bot reads it and prices each create (and each cold start) against it *before* leasing a port or a PVC — so hitting it is an immediate refusal naming what ran out, not a five-minute wait for a pod admission was never going to accept. One bot pod's worth of headroom is withheld from every game admission so the bot's own rolling update can't be starved by the servers it hands out ([ADR-027](docs/decisions/027-namespace-capacity-preflight.md)).
- **Agones** — the substrate, underneath: lifecycle, dynamic port allocation, health. Not hand-rolled, and not the differentiator.
- **Two config tiers** — *per-game* (catalog in `games/`, version-controlled, gated, Flux-deployed) vs. *per-instance* (on the server's PVC, mutated live by the agent, never in git). Per-instance covers the game's own config files plus two reserved files the supervisor owns: an env overlay at `.grizzly/env`, layered into the game process at each launch through an allowlist the catalog declares — that's how Minecraft mods, datapacks, and the JVM heap are set ([ADR-012](docs/decisions/012-per-instance-env-overlay.md)) — and, for the Thunderstore/r2modman games, a mod list at `.grizzly/mods` the supervisor resolves into an installed set before each launch, staged for the game image's own hook to mirror into its loader ([ADR-031](docs/decisions/031-supervisor-reconciles-thunderstore-mods.md)). That list is the *group's* modpack rather than this server's install set — a `client:` line names a package that belongs on the players' machines and is never installed here — because a modpack moves in and out as an r2modman **profile code**: `import_profile` adopts one, and `export_profile` mints one describing what the server runs so a player's versions match by construction. A minted code is permanent and unrevokable, so it is cached beside `resolved` and reused until the set changes, which is what lets export sit at read-only tier ([ADR-033](docs/decisions/033-r2modman-profile-codes.md)). Because that list is rewritten wholesale, an import reports the **diff** against what the server declared — added, removed, version-changed, side-changed — and `update_mods` bumps every pinned entry in one call rather than a `mod_info` round per mod, taking a required `check`/`apply` mode because "any updates?" and "update them" are the same sentence ([ADR-040](docs/decisions/040-a-mod-list-change-is-legible.md)). The other half of "the mod list is the whole truth" is that a removed mod's **settings** go too: the loader writes those at runtime and the image's hook deliberately never overwrites them, so the supervisor attributes each one by the plugin GUID in its header (the same literal is embedded in the assembly that declares it) and deletes the unclaimed ones — once the game reports ready, so a mod list the bot is about to roll back never loses its settings ([ADR-039](docs/decisions/039-sweep-orphaned-mod-settings.md)). Which registry a game's mods come from is an annotation on its `GameServer` template, read by the bot at catalog load; the per-game workflow reaches Gary through a `game_guide` tool rather than the system prompt ([ADR-032](docs/decisions/032-game-guidance-loads-on-demand.md)). **CPU and memory are the third tier**: catalog-declared per game, overridable per instance, stored in Postgres (`instance_sizing`) and re-applied at every render so a resize survives a stop/start. Gary changes them with `set_resources` at manager tier, which patches the running pod through `pods/resize` — live, no restart, nobody disconnected — and refuses values outside a hard clamp rather than silently adjusting them. See [ADR-021](docs/decisions/021-in-place-resource-resize.md).

This is a gated first-party app under the ADR-020 delivery model: root `gate-config.json` honest map, gate-signed images, Flux renders `deploy/`. Agones standup and the agent's guardrails live in `cluster/` **here** (so the gate vets them), not in `grizzly-platform`. `grizzly-platform` keeps only the Flux registration and the edge port-range forwarding (**7000–7099**, UDP+TCP, over the `wg0` tunnel).

### Repo layout

- `crates/` — Rust workspace. `bot/` (Gary's agent loop + the Discord shim he's reached through + Agones client + config loader, binary `grizzly-gameservers`), `supervisor/` (in-pod process supervisor baked into game images — Gary's hands inside a pod, binary `grizzly-supervisor`), `control-api/` (the wire contract shared in both directions: the bot's control client ↔ the supervisor's control server, plus the reverse in-game trigger path where the bot is the server and the game pod is the client — also the source of truth for the route-path constants like `FS_READ_PATH` and `CONTROL_PORT`, so reach for those instead of a bare literal). All model-visible prompt text is compiled into typed Rust accessors by `grizzly-agent-prompts`, a `bot`-crate build/dev dependency rather than a workspace member — see "Prompt Library" below. Strict lints live in `[workspace.lints]` at the root `Cargo.toml`; each crate opts in with `[lints] workspace = true`.
- `deploy/` — Helm chart Flux renders (the bot/agent workload, plus the admin panel's Service/Ingress when `bot.adminPanel.enabled`).
- `cluster/` — Agones standup, agent guardrails (namespace/RBAC/NetworkPolicy), Kyverno gate-scope docs (bot-scoped enforcement; full-namespace carve-out tracked as a follow-up). The bot's RBAC is create/delete only, with one exception: `patch` on `pods/resize`, the narrow subresource behind live resizing ([ADR-021](docs/decisions/021-in-place-resource-resize.md)).
- `games/` — per-game base GameServer/Fleet templates.
- `docs/design/` — design of record; `docs/decisions/` — ADRs as decisions resolve; `docs/operations/` — how to operate a shipped subsystem (the ADR's "why" companion); `docs/archive/` — superseded documents, including designs whose work is complete.

## Running & configuring the bot

The bot ships through gated CI: every push to `main` builds the image, gate-signs it, pins the digest into `deploy/values.yaml`, and Flux reconciles the Deployment (see `.github/workflows/deploy.yml` and the Git Workflow section). **That deployed instance is the one you exercise** — a bot change is verified once it's live on the cluster, not off-cluster. Watch a rollout with `gh run list --branch main` / `gh run watch <id> --exit-status`; don't `kubectl rollout restart` the bot (the digest is already pinned, so a manual restart just re-pulls the same image and races Flux).

**Settings precedence.** Most operator-tunable values resolve **database override > environment variable > compiled default** ([ADR-024](docs/decisions/024-runtime-settings.md)). `crate::settings` owns the catalog (`SettingKey`, a closed enum whose exhaustive matches make an unwired key a compile error) and serves one whole-snapshot `ArcSwap` that consumers read *at the point of use* — never copied into a field, since a stored copy is how a hot setting quietly stops being hot. The env vars below are therefore the **deploy-time default**, not the last word, and a deploy with no stored overrides behaves exactly as it did before the table existed. Secrets are deliberately not settings: they stay in 1Password behind ESO, so *whether* Gary is configured is a startup fact while *which model he drives* is tunable. The six background-cycle cadences are recorded-now/in-force-next-boot, because each is baked into a ticker when its task spawns.

**Config the bot reads** — in the cluster these arrive as secrets via ESO from 1Password (the `onepassword` ClusterSecretStore); for an off-cluster run they come from the repo-root `.env` via dotenvy: `DISCORD_BOT_TOKEN` and `GAMESERVERS_ADMIN_USER_IDS` (comma-separated Discord user ids — the **cross-guild operator seed**; admins in every guild). The bot is multi-guild (commands register per-guild on join), so there's no `DISCORD_GUILD_ID`. Per-guild access is set at runtime with `/config` and stored in Postgres — two tiers: **admins** (full control; `/config admin-role` / `admin-user`) and **managers** (day-to-day lifecycle + Gary file edits, but not destroy/restore/config; `/config manager-role` / `manager-user`). There's no `GAMESERVERS_ADMIN_ROLE_ID` any more; the guild **owner** is an implicit admin so a fresh guild is usable via `/config` with no env change. Gary (the ops agent) needs `OLLAMA_API_KEY` — without it he replies that he isn't configured; optionally override `OLLAMA_BASE_URL` / `OLLAMA_MODEL` (defaults to Ollama Cloud and `glm-5.2`). Gary's conversation is **per-channel, not per-person** — everyone in a channel shares one transcript, each message is attributed to its speaker and tier, and the tool set is recomputed per message from whoever just spoke; a confirmation button can be clicked by anyone at the required tier, which is how an admin approves a friend's destructive request ([ADR-020](docs/decisions/020-channel-bound-conversations.md)). In-game chat stays per-player. A turn already running can be stopped with **`/gary-stop`**, scoped to the channel and ungated like `/new-session`: the loop races the token against each model call and re-checks it between rounds, so a stop lands on the model — where a runaway spends its time — while a tool already in flight always finishes, because a config write with its verifying restart abandoned is worse than waiting one more round. A stopped turn is its own ending rather than an escalation: nobody is paged, it lands in `events` as `kind='turn_cancelled'`, and it still reports what it spent ([ADR-037](docs/decisions/037-stopping-a-running-turn.md)). No-mention **home channels** (`/gary-home`), per-guild admin `/config`, the **product event log**, and **occupancy sampling** need `DB_PASSWORD` (the foundation-Postgres credential; `DB_HOST`/`DB_PORT`/`DB_NAME`/`DB_USER` default to the R730xd `grizzly_gameservers` role/DB) — omit it and the bot degrades gracefully (home channels off, admin config fail-closed to operators + guild owner, no events recorded, sampler never starts) while mentions/DMs/slash commands keep working. Gary's **deferred-task queue** (`run_when` — "wait until {startup|empty|idle} for {server}, then do {task}", non-blocking, batched) needs `REDIS_PASSWORD` (the shared foundation kv-cache/Valkey credential; `REDIS_HOST`/`REDIS_PORT`/`REDIS_DB` default to the R730xd Valkey at `10.0.0.200:6379` DB 2) — omit it and `run_when` reports it can't schedule (Gary offers to do the thing now instead) while everything else works. Pending waits are durable in Valkey and rebuilt on startup, so they survive a redeploy; detection is bot-side polling of the supervisor's `/status` and `/occupancy` (no supervisor changes). See ADR-007. The **proactive watchdog** (`GAMESERVERS_WATCHDOG` = `on`/`shadow`/`off`, code default `on`, deploy currently pinned `shadow`) closes the reactive gap: detection rides the sampler tick as pure state transitions (crash signals from restart-counter deltas and `ProcessPhase::Crashed`, the Agones `Unhealthy` wreck state edge-triggered off the GameServer object since escalation labels the process `stopped` and the pod dies minutes later, a wedged supervisor from consecutive unreachable `/status` polls, memory pressure from actuals vs the declared limit) and, in `on` mode, a fired incident runs a manager-tier Gary turn in the guild's oldest-registered home channel (operator DM fallback) — deferred-batch shape, one at a time, hourly per-(instance, signal) cooldown, storm-guarded. Every incident lands in `events` as `kind='incident'` whatever its disposition, which is what makes `shadow` the tuning mode. Since detection rides the sampler, no `DB_PASSWORD` also means no watchdog. See [ADR-016](docs/decisions/016-proactive-watchdog.md). **Turn telemetry** exports every model-driven Gary turn to the platform's Langfuse as a trace (generations with token usage — prompt tokens split into the uncached remainder and an `input_cached_tokens` bucket, because Langfuse prices every key it recognises and adds them up, so a cache-inclusive `input` would bill each hit at both rates at once ([ADR-042](docs/decisions/042-cached-input-is-its-own-bucket.md)) — tool observations with verbatim arguments/results, plus `escalated`, `escalation_kind`, `cancelled`, and `guardrail` outcome scores — `escalated` is a boolean on every turn so the escalation-rate denominator holds, `escalation_kind` is categorical and only present when a turn escalated, naming which exit it took, and `cancelled` is a second every-turn boolean kept deliberately separate so an operator stopping a turn never counts as Gary giving up); it needs `LANGFUSE_PUBLIC_KEY`/`LANGFUSE_SECRET_KEY` (`LANGFUSE_HOST` defaults to the in-cluster Service) — omit them and the subsystem logs one disabled line and changes nothing else. `GAMESERVERS_RELEASE` carries the image digest the chart pins, stamped on every trace as the release (and, since `grizzly-agent-prompts` compiles model-visible text into the binary, the prompt version). Traces are tagged by environment — `production`, `operator`, `development` — so harness traffic never counts as product signal. See [ADR-013](docs/decisions/013-langfuse-span-construction.md) and `docs/archive/data-collection/design.md`. The **product event log** is the second data plane: an append-only `events` table recording every lifecycle, backup, escalation, and turn-cancellation action with the actor who took it and the surface it came from (`command` for a slash command, `agent` for a Gary tool, `deferred` for a `run_when` batch, `operator` for harness traffic, `system` for automatic cycles), plus a `known_guilds` table so `guild_joined` fires on a real join and not on every gateway reconnect. Events are emitted by the layer that *executes* an action, never inside the shared `agones` functions — those double as internal building blocks. See [ADR-014](docs/decisions/014-product-event-log.md). The same plane carries an `occupancy_samples` time series: a background sampler (`GAMESERVERS_SAMPLE_INTERVAL_MINUTES`, default 5) polls each *running* server's supervisor `/status` and `/occupancy` and appends player count and restart counter per instance per tick — integrated into playtime, maxed into peak concurrency, differenced into crash frequency. A null player count means **unknown, never zero** (a game with no working occupancy probe, or an unreachable console); paused instances aren't sampled at all. The same tick also appends a `resource_samples` row per *existing* instance (running, paused, or shut down — a paused pod still reserves RAM, a shut-down instance still holds its PVC): actual whole-pod CPU/memory from metrics-server, declared requests/limits off the live pod spec, and PVC capacity — raw quantities only, never money; the market-proxy USD rate card lives in the cost query files and prices at read time (see [ADR-015](docs/decisions/015-market-proxy-cost-plane.md)). Both planes are also browsable in the platform's Metabase at `analytics.grizzly-endeavors.com` — `just seed-metabase` pushes `scripts/queries/*.sql` up as saved questions and a dashboard, reading through a `SELECT`-only role. **Turning either plane into a number is [`docs/operations/data-collection.md`](docs/operations/data-collection.md)** — the Langfuse read paths (this deployment is v4 `events_only`, so most of the documented read API is gone), the model-price setup, the query runners (`just metric <name>`, `just db-query <name>`, backed by `scripts/queries/`), and the traps that make a naive query wrong. Read it before writing an ad-hoc query against `events` or `occupancy_samples`.

**Who the bot serves, and how fast.** `GAMESERVERS_GUILD_ALLOWLIST` (comma-separated guild ids, synced from 1Password as `discord-bot/guild_allowlist`) is the admission gate: **empty serves every guild**, which is the historical posture and the safe default for an un-configured deploy; populated, an unlisted guild is registered with an empty command set and Gary ignores its messages. Independently, every model-driven turn passes an admission gate in `crates/bot/src/throttle.rs` — a per-subject token bucket (`GAMESERVERS_TURN_BURST`, default 10; `GAMESERVERS_TURN_REFILL_SECONDS`, default 30) plus a global concurrency permit (`GAMESERVERS_MAX_CONCURRENT_TURNS`, default 6). The subject is the Discord user, or — for in-game `@Gary` — the (server, player) pair, because that surface carries no Discord identity and is reachable by anyone who can connect to a public game port. A turn is the unit that costs money, so the gate sheds rather than queues: a queued flood is still a billed flood.

**Beta signups.** The public landing page (`gary.grizzly-endeavors.com`, built in the separate `gary-landing` repo) POSTs form submissions to a bearer-gated intake route on the agent endpoint — `GAMESERVERS_SIGNUP_TOKEN`, a third credential distinct from the in-game and operator tokens, whose entire capability is appending a signup. The landing service holds no database credential and the bot stays the sole writer; the route mounts only when the token *and* a database are both present, since accepting a submission with nowhere to record it loses somebody who asked for access. One form carries two independent opt-ins (**beta**, an approval queue; **updates**, a mailing list with no approval step), and pending-ness is the *absence* of a status rather than a `'pending'` string. Approving mails the applicant a bot-invite link and is deliberately **not** the moment access is granted — that's admitting their guild on the panel's Guilds page after the bot joins, because Discord offers no way to restrict who installs a public bot. **Approval mail needs `GAMESERVERS_SMTP_PASSWORD`** (the `gary@grizzly-endeavors.com` Stalwart submission account; `GAMESERVERS_SMTP_HOST`/`PORT`/`USER` and `GAMESERVERS_MAIL_FROM` carry defaults) — omit it and approvals are still recorded but nobody is told, with a Resend queue on the page to catch up later. Mail never retries and latches off for the life of the process on a refused credential, because submission hairpins out to the VPS and a retried bad credential gets the household's address banned. See [ADR-029](docs/decisions/029-signup-intake-through-the-bot.md), [ADR-030](docs/decisions/030-transactional-mail-fails-closed.md), and [`docs/operations/signups.md`](docs/operations/signups.md) — **read the operations doc before the page goes public**, because an empty guild allowlist serves *every* guild and that becomes an open door the moment invite links are handed out.

The bot's calls to a game pod's supervisor carry `SUPERVISOR_CONTROL_TOKEN`, a shared bearer token the supervisor requires on every control route (the `grizzly-gameservers-control` Secret, read by both halves). It is optional on both sides — absent, the control API is guarded by the `game-servers` egress leash alone, which is the pre-token posture — so a half-rolled deploy can't lock the bot out of every server. The credential rides a distinct `ControlClient` type rather than the bot's general `reqwest::Client`, because that one also reaches Ollama, Modrinth, Thunderstore, and Langfuse; the type is what stops the token leaving the cluster. See [ADR-017](docs/decisions/017-supervisor-control-auth.md).

**Super-admin panel** — <https://gameservers-admin.grizzly-endeavors.com>, gated by Authentik (`grizzly-admins`). The operator's browser view of the service and the writer for everything in the settings catalog: guild admission, per-guild caps, the model, the throttle, the transcript bounds, the watchdog mode, the per-server bounds. Also drives servers (stop/start/restart/shut down/destroy/back up) and browses the event log. It runs **in the bot process on its own port** (9361), because the agent port must stay reachable by game pods while the panel trusts forward-auth headers — `cluster/guardrails/bot-ingress.yaml` admits only ingress-nginx to it, and that policy is the first *ingress* policy on the bot pod (so it flips that pod to default-deny inbound; every legitimate flow is enumerated there). Unset `GAMESERVERS_ADMIN_PORT` means the listener never binds. Operating it — including the live-vs-restart split and the break-glass path — is [`docs/operations/admin-panel.md`](docs/operations/admin-panel.md). See [ADR-026](docs/decisions/026-super-admin-panel.md); the Authentik half (proxy provider, application, policy binding, and the embedded-outpost provider list) lives in grizzly-platform's blueprints.

**Driving the deployed bot** — you can exercise the live instance yourself; you do not need a human at a Discord client. The bot serves three bearer-gated operator routes on its agent endpoint, wrapped by `just operator` (see the `drive-the-bot` skill for the workflow):

- `just operator tool <name> ['<json>']` — run one of Gary's tools directly, no model in the loop. **This is the default path**: deterministic, fast, and free.
- `just operator gary "<prompt>"` — run a full Gary turn. Reach for it only when Gary's *reasoning* is what's under test. Returns the reply, a trace of every tool the model called and what each returned, and what the turn cost — per model call and summed: token usage, finish reason, and measured latency. A token count of `?` means the provider reported none, never that the call was free.
- `just operator tools [--access <tier>]` — the tools a tier may call, with their JSON schemas. Run this rather than guessing an argument name.

`--access read-only|manager|admin` picks the tier to act at (default `admin`), so you can verify what a lower tier sees without a second Discord account. `--confirm` pre-approves the destructive confirmations that would otherwise block on a button click — admin-only, and every use posts a visible audit line into the channel. The script port-forwards the ClusterIP Service, reads the token from the ESO-synced `grizzly-gameservers-operator` Secret, and reads `GAMESERVERS_OPERATOR_CHANNEL` from the gitignored repo-root `.env` (Discord ids never enter this public repo). `just bot-logs-deployed` tails the live pod. See [ADR-011](docs/decisions/011-operator-harness.md).

**Off-cluster local run** — rarely needed now (the default is to test once deployed), so reach for it only to iterate/debug the bot away from the cluster: `scripts/local-bot.sh {start|stop|restart|status|logs}` (or `just bot-start` / `bot-stop` / `bot-logs`) runs the binary against your current kubeconfig context, reading the vars above from `.env`. It overrides `GAMESERVERS_CATALOG_DIR` to the in-repo `games/` (the compiled default is the in-container path), builds and launches the binary detached, writes the pid to `target/local-bot.pid`, and streams output to `target/local-bot.log`. After editing `.env` you must `restart` for changes to take effect.

The bot ships through CI (above), but the **per-game supervisor runs in-pod**, so iterating on it means building and pushing the composite game image rather than waiting on a deploy. Use `scripts/push-game-image.sh [game] [tag]` (or `just game-push minecraft dev`): it builds `games/<game>/Dockerfile` (cargo-chef-cached, so a source-only change rebuilds in seconds), port-forwards the in-cluster registry (`registry.registry.svc.cluster.local:5000`, plain HTTP), and pushes through `localhost:5000` (Docker treats localhost as insecure). The `:dev` catalog tag pins `imagePullPolicy: Always`, so a freshly created (or cold-started) server re-pulls. Game images aren't gate-signed or auto-deployed by CI yet, so `game-push` is the operative dev loop for a supervisor change — the durable CI build (`.github/workflows/deploy.yml`) currently covers the bot image only.

## Prompt Library

Every piece of prose the software *sends* to a model — system prompts, user-turn framing, tool names, tool descriptions, tool parameter descriptions, and the fixed prose of tool *results* fed back into the conversation — lives as a Markdown file under a consuming crate's `prompts/` tree (`crates/<crate>/prompts/<feature>/<Id>.md`, feature-nested — subdirectories are organizational only, ids stay flat and globally unique), compiled by [`grizzly-agent-prompts`](https://github.com/Grizzly-Endeavors/grizzly-agent)'s build step into typed Rust accessors. The bot crate's `build.rs` calls `grizzly_agent_prompts::emit`; the file format, YAML conventions, and maintenance contract below are unchanged from the in-repo `prompt-lib` crate this replaced (its design doc is `docs/archive/prompt-lib/design.md`, kept for historical context on the format's rationale). Computed data (lists, ids, counts) is **not** prompt text: it enters a prompt through a `{{variable}}`. A short value-level fallback for absent data (rendering a missing game as `unknown game`, an empty list as `(none)`) stays in Rust but must be described in that variable's `contents` annotation, so the tuning surface still shows it. Out of scope: what the software does with model *output*, and text shown to human users.

**The maintenance contract (load-bearing).** Any change to code that renders a prompt, adds or removes a prompt variable, or moves a call site **must update that prompt file's `annotations` in the same change**. Variable coverage is enforced at build time and `used_by` accuracy by the annotations test below; `sent_when` and `reasoning` are not machine-checked and are maintained on trust — a stale one is caught exactly when someone opens the file to tune it, so keep them honest. Prompt bodies and tool/parameter descriptions are the **human's tuning surface**: you may *create* them, but do **not** rewrite existing model-visible text without being asked.

**YAML conventions** (keep files uniform): double-quote any value containing a colon; use a block scalar (`>-`) for multi-line prose in annotations and descriptions; an `id` is PascalCase matching `([A-Z][a-z0-9]*)+` (no acronym runs — `HttpProxy`, never `HTTPProxy`), globally unique, and equal to the filename stem — the id *is* the generated type name, so grepping it finds the file and every call site; `used_by` entries are `{file, function}` at file-and-function granularity, **never line numbers**, with `file` relative to the crate's `src/` (e.g. `discord/gary/tools.rs`, no `src/` prefix).

**The annotations test.** Each consuming crate carries exactly one integration test that cross-checks annotations against the source tree — it blocks a merge, not compilation, since a stale cross-reference shouldn't stop a mid-refactor build:

```rust
#![expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live at crate root by cargo convention"
)]
use std::path::Path;
use grizzly_agent_prompts::verify_annotations;

#[test]
fn prompt_annotations_are_current() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    verify_annotations(&root.join("prompts"), &root.join("src"))
        .expect("prompt annotations are stale — fix the named prompt file's used_by entry");
}
```

It fails if a `used_by` entry names a source file that doesn't exist or that lacks the id or the named function, or if a prompt id appears in no source file (an orphan). A consuming crate declares three git dependencies on `grizzly-agent`, all pinned to the same `rev` (two copies of core would make generated `ToolSpec`s fail to type-check against each other): `grizzly-agent-prompts` in `[build-dependencies]` with `features = ["codegen"]`, `grizzly-agent` in `[dependencies]` with `default-features = false`, and `grizzly-agent-prompts` again in `[dev-dependencies]` with `features = ["verify"]`.

## Naming

- **Domain-specific names**: prefer descriptive names that match the domain (`send_chat_completion` over generic `run`, `spawn_widget_window` over `handle`).
- **Common abbreviations OK**: `cfg`, `dir`, `msg`, `ctx`, `cmd` are fine; avoid obscure ones.
- **Semantics must match logic**: structs, enums, and functions should make it abundantly clear what they do. Avoid vague names and catch-alls. If the logic doesn't match the semantics of the name, refactor the logic or rename it — don't let the name lie.

## Error Messages

- **Always include context**: `"failed to parse config at {path}"` — not just `"parse error"`.
- **Lowercase, no trailing period** — Unix style, chains cleanly with context wrappers (`anyhow::Context`, `errors.Wrap`, `Error.cause`, etc.).
- **User-facing vs developer-facing**: developers get structured, chained context; end users get plain-language, actionable messages. These are different audiences — don't conflate them.

## Comments

- **Explain *why*, never *what***. The code shows what; comments exist for non-obvious reasoning — hidden constraints, subtle invariants, workarounds for specific bugs, behavior that would surprise a reader.
- **No comments that restate the code**: `// increment counter` on `counter += 1` is noise. Delete it.
- **Doc comments**: one-line summary for public items. Expand only when behavior is non-obvious (error modes, performance notes, thread-safety). Don't narrate parameters that are already named.
- **No planning / decision / analysis comments in shipped code**. Those belong in docs, commit messages, and PR descriptions, not in the file.

## Visibility

- **Private-first**: start with no visibility modifier (or the language's equivalent: unexported, module-private, file-scoped). Add `pub` / `public` / `export` only when there's a consumer that demands it.
- **Treat public as a commitment**: once something is public, it's API. Future changes cost consumers. The cheapest API change is the one you never made public in the first place.
- Prefer package-internal visibility (`pub(crate)`, internal module re-exports) over fully public when the consumer is inside the same project.

## Testing

**Testing is a first-class operation — NEVER skip test implementation.**

- Every pure module ships with tests on the day it lands. "I'll add tests later" is how untested code accumulates.
- **Unit tests** live next to or inside the code they test (language-dependent placement: inline `#[cfg(test)]` module in Rust, `_test.go` sibling in Go, `test_*.py` sibling in Python, etc.).
- **Integration tests** live in a dedicated directory (`tests/`, `integration/`, or language idiomatic equivalent).
- **Always run tests with a quiet flag** where available (`cargo test --quiet`, `pytest -q`, etc.) — suppresses per-test noise, surfaces failures and summaries.
- The shell (IO, framework, GUI) is usually not unit-tested; it's exercised via integration tests or manual verification. But the *logic* reachable from the public API should be testable without the shell.
- Don't test implementation details — test behavior. A refactor that preserves behavior shouldn't break tests.

## Observability — No Silent Failures

**Every failure must be visible.** This is non-negotiable.

### User-facing: clear, actionable messages

- Assume end users are non-technical. Error messages must explain what went wrong and what to do next — not expose internals.
- Plain language: `"Couldn't connect to the server. Check your internet connection and try again."` — not `"TCP connection refused on port 443"`.
- Partial failures (e.g., 2 of 3 items synced) must tell the user what succeeded, what failed, and whether action is needed.
- Never show raw error types, stack traces, or module paths to end users.
- If an operation fails silently with no user impact, it still needs a log.

### Developer-facing: rich, structured diagnostics

- Every error path produces a log entry with enough context to diagnose without reproducing.
- Use structured fields — `error!(error = %e, path = %path, "failed to read config")` — not prose string interpolation.
- Chain error context at each layer so the log shows the full causal chain (`anyhow::Context` in Rust, `fmt.Errorf("...: %w", err)` in Go, exception chaining in Python).
- Pick the right level: `error` (operation failed), `warn` (recoverable/degraded), `info` (major lifecycle event), `debug` (state transitions), `trace` (payloads, per-tick).

### Avoid log spam

- Do not log every retry individually — log once at `warn` when retries start, and once when they resolve or exhaust.
- Do not log routine successful operations ("heartbeat ok", "connection alive"). Absence of errors is the signal that things work.
- Anything that would fire on every frame, poll tick, or event under normal conditions belongs at `trace` at most, never `info`.

## Git Workflow

**Single-branch model**: all work lands on `main`.

### Day-to-day

1. Create a feature branch from `main` with a prefixed name (`feat/`, `fix/`, `refactor/`, `docs/`, `ci/`, `chore/`).
2. **Commit frequently** — especially during large multi-phase tasks. Pre-commit hooks enforce fmt, lint, and tests.
3. Wrapup: check for anything unfinished, update docs/guides, create migration guides for breaking changes.
4. Push the branch and merge into `main`.

### Rules

- **Never use `git -C`** — the shell is already in the project root. Use plain `git` commands.
- **Use `git commit -m "message"` only** — no HEREDOC, no `$()`, no `cat <<EOF`. These alternate formats fail on this author's setup and will be rejected.
- **Hook bypass (`--no-verify`) is FORBIDDEN.** If a hook fails, fix the underlying issue.
- **First-line commit conventions**: ≤72 chars, lowercase, imperative mood, conventional prefix (`feat:`, `fix:`, etc.), no trailing period.
- All changes must be committed before giving the user a completion summary.

## Lint Configuration

Clippy is configured with strict denies — not warnings. The lint block in `Cargo.toml` is intentional and not to be relaxed without explicit approval.

### Why strict

These rules are strict because this project is primarily developed with AI coding agents, and agents will take every shortcut that isn't explicitly denied. Warnings get ignored; only hard errors change behavior. The strictness is a substitute for the discipline a human developer would bring naturally.

### What's denied and why

- **`unwrap_used`, `expect_used`, `panic`, `get_unwrap`**: no panics on untrusted input or error paths. Use `?`, `anyhow::Context`, or explicit handling.
- **`todo`, `unimplemented`, `dbg_macro`**: no incomplete or debug code ships.
- **`exit`**: only `main.rs` may call `std::process::exit`. Library code returns `Result`.
- **`indexing_slicing`, `string_slice`**: use `.get()` / slice-returning methods that produce `Option`.
- **`map_err_ignore`, `let_underscore_must_use`**: no silent error swallowing.
- **`missing_errors_doc`, `missing_panics_doc`, `must_use_candidate`**: public API must document its failure modes and must-use returns.
- **`allow_attributes`, `allow_attributes_without_reason`**: every suppression uses `#[expect(..., reason = "...")]`, never `#[allow(...)]`. `#[expect]` warns when the suppression goes stale.
- **`tests_outside_test_module`**: tests live in `#[cfg(test)] mod tests` blocks (or in `tests/` at crate root for integration — those files need the lint-level escape at the file top).
- **`wildcard_enum_match_arm`, `shadow_unrelated`**: no lazy match catch-alls, no surprise variable shadowing.
- **`rc_buffer`, `rc_mutex`**: antipatterns.
- **`clone_on_ref_ptr`, `format_push_string`, `redundant_type_annotations`**: code quality.

### Relaxed

- `pedantic` is at `warn` with `priority = -1`: pedantic baseline, but deny would break builds on toolchain upgrades that add new lints.
- `module_name_repetitions = "allow"`: pervasive pattern in this codebase style.

### Test modules

Test code may freely use `.unwrap()` / `.expect()` / `panic!` / `dbg!` — the root `clippy.toml` exempts them in tests (`allow-unwrap-in-tests` etc.), mirroring grizzly-gate's own policy. Do **not** add `#[expect(clippy::unwrap_used, ...)]` to test files: the gate exempts the lint, so the expectation never fires and becomes an unfulfilled-expectation hard error. (`tests_outside_test_module` is *not* test-exempt — see the integration-test boilerplate below.)

### Unsafe

`unsafe_code = "deny"` (not `forbid`) — `deny` allows scoped `#[expect(unsafe_code, reason = "FFI boundary")]` at the item level when genuinely needed. `forbid` can't be overridden item-level.

## Async

- **Tokio is the default runtime.** `#[tokio::main]` in `main.rs`, `tokio::spawn` for background tasks, `tokio::select!` for concurrent I/O supervision.
- **Async-first**: use async for I/O; fall back to sync only for CPU-bound or genuinely trivial operations.
- **No blocking calls inside async functions.** Use `tokio::fs`, `tokio::process`, etc. If a blocking call is unavoidable, wrap it in `spawn_blocking`.
- **Shared state**: wrap in `Arc<T>` with thread-safe interior mutability (`DashMap`, `Mutex`/`RwLock` when contention is low, channels for message passing).
- **Graceful shutdown**: listen for `SIGINT`/`SIGTERM` via `tokio::signal`; let the `tokio::select!` in the main loop fall out on signal and drain in-flight work.
- **Tracing init happens before anything else async starts** — `EnvFilter::from_default_env()` reads `RUST_LOG` at startup.

## Error Handling

- **`anyhow::Result<T>`** at subsystem boundaries and inside binaries. Chain context with `.context("failed to load user settings")` so logs show the causal chain.
- **`thiserror` enums** for domain errors that callers match against or that need to map to specific outcomes (HTTP status codes, exit codes, user-facing error kinds).
- **`anyhow` wraps `thiserror`**: domain errors bubble up as typed errors; boundaries widen them to `anyhow::Error` with added context.
- **`main.rs` is the only place that maps `Result` to exit code.** Every other function returns `Result`; the `exit` lint is denied elsewhere.
- **Error messages**: lowercase, no trailing period, context-first (`"failed to parse config at {path}"`).
- **No `.unwrap()` / `.expect()` outside of tests** — the lints deny them. If you genuinely know a value is present, use `.expect("reason — invariant explanation")` inside a scoped `#[expect(clippy::expect_used, reason = "...")]` with a real reason.
- **No silent error swallowing.** `let _ = ...` on a `Result` is denied; so is `.map_err(|_| ...)`. Every error either propagates or gets logged with context before being handled.

## Module Layout

- **`mod.rs`** primarily contains declarations and curated `pub use` re-exports. Module-level coordination logic is fine when it belongs there; gratuitous plumbing is not.
- **Group related types in one file** (e.g., `Message`, `Role`, `ToolCall` together in `llm/types.rs`) rather than one-type-per-file.
- **Shell vs core split**: IO-bound / framework-bound code (HTTP handlers, GUI callbacks, Bevy systems, event loops) lives in a thin *shell* layer that calls into *pure* modules where all the logic is. The shell is usually not unit-tested; the pure modules are.
- **Binary vs library**: even binary crates benefit from a thin `main.rs` that delegates to a library module — makes the logic testable and keeps `main.rs` at the "load env → init tracing → dispatch → exit code" skeleton.
- Test placement is covered in its own section (see "Test Organization (Rust)").

## Test Organization (Rust)

Testing is a first-class operation — NEVER skip test implementation. Always run `cargo test --quiet`; never plain `cargo test` (the `--quiet` flag suppresses per-test noise and surfaces failures + summary).

### Unit test file layout

Unit tests do **not** live at the bottom of the impl file. They live in a sibling `tests/` subdirectory, loaded via a `#[path]` module declaration at the bottom of the impl file. This keeps impl files short and prevents a three-line edit from dragging hundreds of lines of test noise into the LLM context window.

Per-module rules:

- `src/foo.rs` → tests at `src/tests/foo.rs`, declared in `src/foo.rs` as:
  ```rust
  #[cfg(test)]
  #[path = "tests/foo.rs"]
  mod tests;
  ```
- `src/foo/bar.rs` → tests at `src/foo/tests/bar.rs`, declared in `src/foo/bar.rs` as:
  ```rust
  #[cfg(test)]
  #[path = "tests/bar.rs"]
  mod tests;
  ```
- `src/foo/mod.rs` → tests at `src/foo/tests/foo.rs` (named after the module, not `mod.rs`), declared in `src/foo/mod.rs` as:
  ```rust
  #[cfg(test)]
  #[path = "tests/foo.rs"]
  mod tests;
  ```

The `#[path]` attribute makes the loaded file a **child** of the impl module, so `use super::*;` retains full access to private items — no visibility inflation.

### Unit test file boilerplate

A sibling test file needs no lint preamble — `.unwrap()` and friends are exempt in tests via the root `clippy.toml`. Just open with the imports:

```rust
use super::*;
// ...
```

### Integration test boilerplate

Every `tests/*.rs` file at crate root must start with:

```rust
#![expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live at crate root by cargo convention"
)]
```

This attribute is required because `clippy::tests_outside_test_module` is denied project-wide and fires on top-level `#[test]` functions — which are exactly what integration tests are. It is *not* covered by the test exemptions in `clippy.toml`, so it stays. (Unwrap/expect/panic/dbg need no suppression — those are exempt in tests.)

### What gets tested

- **Pure modules** (parsing, validation, state transitions, path manipulation, error construction): every pure module ships with tests on the day it lands.
- **Shell modules** (GUI callbacks, HTTP handlers, framework integrations, event loops): usually not unit-tested. Exercised via integration tests or manual verification.
- **Logic reachable from `lib.rs`**: testable without the shell. If you find yourself adding logic to a shell module, move it into a pure module first and let the shell call the validated result.

### Env var handling in tests

Rust 2024 made `std::env::set_var` / `remove_var` `unsafe`. Do not use them in tests. Instead, structure env-reading functions as a dual pair:

- Public zero-arg function that reads the real process env (e.g., `wayland_session()`).
- Public `_from_env` sibling that takes an `EnvLookup<'_>` closure (e.g., `wayland_session_from_env(get_env)`).

The public function calls the sibling with `&|k| std::env::var_os(k)`. Tests construct closures with fixed keys: `(k == "WAYLAND_DISPLAY").then(|| OsString::from("wayland-0"))`.

### Rationale

This layout is optimized for LLM agent workflows where the default "read the whole file" cost is multiplied by every edit. Sibling test files preserve Rust's native test ergonomics (private access, `cargo test` runs them automatically) while dramatically cutting read-time noise on mature modules.

