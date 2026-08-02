# Data Collection for Productization — Design

> Systems level only. No file or line references. This document must stand on its own: it will be implemented later in fresh sessions that have only this doc, phases.md, and the codebase — not the conversation that produced it.

## Goal & context

The [productization assessment](../../product/productization-assessment.md) commits to running this service as an instrumented experiment, and the instrumentation does not exist. The assessment names four numbers; three are in scope here:

1. **Escalation rate** — how often Gary gives up versus fixes the thing.
2. **Cost per agent turn** — token spend per interaction, the COGS driver.
3. **Retention / stickiness** — do non-friend users keep using their servers.

The fourth (RAM reclaimed by right-sizing) is **explicitly deferred**: Gary has no right-sizing capability today, so there is nothing to measure yet. Nothing in this design should preclude adding it later, but nothing here builds toward it either.

The assessment also directs: *"Start capturing traces now. Log the full record of successful fixes — symptom, files read, edit made, outcome — not just the facts learned."* That trace corpus is simultaneously the fine-tuning dataset, the memory-system evidence base, and the B2B sales collateral, and it cannot be reconstructed retroactively.

Today the bot captures almost none of this. The model client discards the token-usage block that already arrives on every completion response. No turn, tool call, or model call is timed. Lifecycle outcomes (create, destroy, crash, rollback, restore) are rendered into Discord embeds and dropped. Occupancy is polled in three places and stored in zero. Escalations are a DM and a warning log. There is no correlation id anywhere, no metrics endpoint, no exporter. The platform deployed [Langfuse](https://langfuse.grizzly-endeavors.com) (grizzly-platform ADR-064) precisely to receive the LLM-observability half of this data; nothing sends to it yet.

**The design principle: capture at the choke points the codebase already has, on two planes matched to the two shapes of data.** LLM-interaction data (traces, generations, tool calls, outcomes) goes to Langfuse. Product-usage data (lifecycle events, occupancy, acquisition) goes to Postgres tables beside the bot's existing state. Both planes degrade gracefully to "feature off" exactly like every other external dependency in this codebase.

## Shape

Three subsystems, all inside the bot process. The supervisor, control-api, and game images are untouched.

### 1. Turn telemetry (Langfuse plane)

A telemetry subsystem in the bot crate that records every **model-driven Gary turn** as a Langfuse trace and exports it over OpenTelemetry OTLP.

**Turn context.** Every Gary turn gains an explicit **origin** discriminant — `discord`, `ingame`, `deferred`, or `operator` — set where the turn is constructed, replacing today's after-the-fact inference. The turn also carries what it already knows (guild, channel, user/actor, access tier, confirmation mode) plus a generated turn identity that becomes the OTel trace id.

**Trace model.** One trace per turn:

- **Root span** — the whole turn: start-to-reply duration, origin, round count (the recorder's own count of generation observations), whether the turn escalated, the reply length. Trace name identifies the turn kind. A turn that aborts on a hard model-endpoint failure still exports its trace — root span marked errored, partial observations kept, no outcome scores — so provider outages stay visible and partial cost isn't lost.
- **Generation observation per model call** — the request messages, the response message, the model name, token usage, finish reason, and latency. This requires widening the model client's response type to stop discarding the `usage` and `finish_reason` fields it already receives and returning them, with measured latency, as a richer completion result. The session loop's completion hook widens to that result type, so every shell carries the data by construction — a shell cannot compile while dropping it.
- **Tool observation per dispatched tool call** — tool name, verbatim arguments JSON, the full result string the model saw, and duration. Captured at the session loop's dispatch point so every surface (Discord, in-game, deferred, operator) is covered by one mechanism. This is what makes a trace a *fix record*: for a config edit, the arguments contain the exact old/new text and the result contains the guardrail verdict.

**Identity attributes.** Langfuse does not inherit trace-level attributes across spans, so the recorder stamps every span with the same identity set: environment, user id, session id, release, and trace-level tags/metadata (origin, guild, game where known, access tier). Session id identifies a conversation *instance*, not the conversation's key: a fresh opaque id is minted whenever the bot's session store starts a new transcript, stored alongside that transcript, and rides along until the session ends (TTL expiry, explicit reset, or process restart). The id is never derived from the store's keying — today sessions are scoped per channel-and-user, but if that scoping later changes (e.g. one shared conversation per channel for natural multi-user chat), telemetry follows automatically because it only ever tags the transcript instance it was handed. One Langfuse session therefore equals exactly one conversation as the bot experienced it, whatever the bot's conversation scoping is — and a session may legitimately contain traces from multiple users. Deferred-batch turns run outside the session store entirely — each batch is a one-shot conversation — so they carry no session id; their origin tag is their grouping. User id on a trace is the author of the message that triggered that turn (Discord snowflake, or the in-game player identity for in-game turns); per-turn attribution inside a shared conversation is exactly what makes a multi-user session readable. Release identifies the exact build: the gate-signed image digest, which the deploy pipeline already pins into the chart and the chart injects into the bot's environment — no crate-version bump discipline required, content addressing does it (off-cluster runs fall back to a dev marker). Because all model-visible text is compiled into the binary by prompt-lib, the digest also *is* the prompt version.

**Full-conversation capture.** Each generation observation records the complete message array sent to the model, and Gary's calls always carry the whole conversation-so-far from the session store — so a single trace already contains the context the model actually saw, and the session id links the turn-traces into the browsable whole. Nothing summarizes or truncates: the corpus keeps what the model kept.

**Environment separation.** Every trace carries a Langfuse environment: `production` for real traffic (Discord, in-game, deferred), `operator` for operator-harness turns, `development` for off-cluster local runs. Local runs have no route to the in-cluster ingestion endpoint, so telemetry is simply off locally unless the developer opts in by pointing the exporter at the public hostname; `development` exists so those opt-in traces stay quarantined. Operator tooling and dashboards filter on it, so test traffic can never pollute product metrics. Direct operator tool invocations (no model in the loop) are not traced at all — they are free, deterministic, and excluded from every metric this design serves.

**Outcome scores.** After a turn completes, the recorder attaches Langfuse scores to the trace via the scores REST API:

- `escalated` (boolean) — on every turn; true when the turn ended in the round-budget escalation path.
- `guardrail` (categorical) — only on turns whose restart carried a pending config change. A closed set of five, mapped from the guardrail flow's verdicts: `healthy` (verified healthy after restart), `rolled_back` (crashed, rollback recovered it), `rollback_failed` (every path where the rollback story ends badly — a second crash after the one rollback was spent, a restore whose follow-up restart didn't come back, or a restore that couldn't be issued at all), `inconclusive` (verification ran but couldn't reach a verdict — readiness timed out or the server landed in a state that answers nothing), and `unverified` (the restart path couldn't run verification at all).

These two scores, aggregated in Langfuse and filtered to `production`, *are* the escalation-rate metric.

**Cost.** The recorder ships token usage; it never computes money. Langfuse holds the model-price definition (a one-time manual configuration per model name) and derives cost per generation, per trace, per day. When the model changes, the price table changes in Langfuse, not in code.

**Export discipline.** Spans buffer in a batching exporter and ship in the background; a turn never waits on telemetry, and an unreachable Langfuse costs visibility, not function. The exporter's flush joins the bot's existing graceful-shutdown drain, so a redeploy — which happens on every merge — doesn't silently drop the buffered tail. Absent credentials disable the subsystem entirely at startup with one log line — identical to how the bot treats a missing database. Export failures log once at warn on state change (not per batch), matching the house no-log-spam rule. 100% of turns are captured — no sampling; traffic is friends-scale, and every trace is corpus.

### 2. Product event log (Postgres plane)

An append-only event store in the bot's existing Postgres database, written through a graceful-degradation façade shaped like the bot's other stores, holding three small tables:

**Events table.** One row per product-meaningful action, emitted at the orchestration layers where the action actually executes (not where it is requested), so slash commands, Gary tools, deferred batches, the operator harness, and automatic cycles all land in the same stream. Each row: timestamp, event type, guild, game, instance (those three each nullable — a catalog row populates what applies to it), actor (Discord snowflake as text, or a system marker for automatic actions), the surface it came from (command, agent tool, deferred, operator, system), and an outcome plus a JSONB details column for event-specific payload (sizes, boot states, failure variants).

The event catalog, closed and enumerated:

| Event | Emitted when | Notable payload |
|---|---|---|
| `server_created` | once per create attempt, when the post-provision readiness wait resolves (immediately, if provisioning itself fails) | outcome, address, time-to-ready (null unless readiness was confirmed) |
| `server_started` / `server_stopped` / `server_restarted` | warm lifecycle actions | outcome variant |
| `server_shutdown` | GameServer deleted, PVC kept | outcome |
| `server_destroyed` | full teardown | outcome |
| `server_archived` / `server_recovered` | archive machinery | size, outcome, boot state |
| `backup_taken` / `server_restored` | backup machinery, manual or auto | size, safety-backup state, boot state |
| `backup_cycle` | scheduled cycle completes | servers, succeeded, failed |
| `escalation` | an escalation is determined (round budget exhausted, or a crash-rollback verdict) — regardless of origin and of whether a notification is deliverable | kind; server and path only when the kind carries them |
| `guild_joined` | first-ever sighting of a guild | guild id |

`guild_joined` requires a persisted known-guilds set — the store's third table — so gateway reconnects (which replay guild events) don't re-fire it. On the first startup after this ships, currently-connected guilds are seeded as known *without* emitting events, so the acquisition timeline starts honestly at deployment instead of backdating every existing guild as a join.

**Occupancy samples table.** A bot-side sampler polls each running server's supervisor on a fixed cadence (minutes, not seconds) and appends: timestamp, instance, game, guild, player count, and the supervisor's process-restart counter. A sample costs two control-API round trips per instance — status and occupancy are separate routes — and the sampler's server enumeration must surface each instance's guild, which the cluster labels already carry but not every existing listing exposes. Player count is nullable and null means *unknown, never zero* — same semantics the deferred-condition logic already enforces. Games whose occupancy probes don't work yet simply record null; those gaps are tracked issues, not this design's problem. From this table: playtime per guild per week (retention), peak-concurrency curves (the unmeasured packing input the assessment flags), and crash frequency (restart-counter deltas). The sampler reuses the existing server-listing and supervisor-client machinery; the supervisor itself is unchanged.

Actor attribution is currently dropped for every lifecycle action; the emission sites receive the acting user and surface as part of this work. Volume is trivial at this scale, so rows are kept indefinitely — the event log is a permanent product record.

### 3. Derivation layer (queries, dashboards, runbook)

No new software — documentation and configuration that turn the two planes into the experiment's numbers:

- **Escalation rate** — Langfuse score aggregation over `production` traces.
- **Cost per turn** — Langfuse cost views over generations, backed by the manual model-price definition.
- **Retention / engagement** — SQL over the events and occupancy tables: distinct active guilds/users per week, playtime per guild, time from `guild_joined` to first `server_created`, archive-and-never-recover as the churn signal.
- **Corpus curation** — the documented path from "good trace in Langfuse" to a dataset item via Langfuse's datasets API, for the future fine-tuning set. Capability documented, pipeline not built.

A short operations document in this repo records the Langfuse project setup, the model-price definition, the environment taxonomy, the score names, and the queries — so the metrics are reproducible by someone who has never read this design.

## Reasoning & alternatives

**Why two planes instead of one.** Langfuse models traces, observations, and scores; it is deliberately not a product-analytics store (grizzly-platform ADR-064 chose it over PostHog-style tools for exactly this shape). Server lifecycle and occupancy are events-by-tenant-over-time — SQL shapes. Forcing either into the other's store produces bad queries on one side or a second analytics deployment on the other. The bot already owns a Postgres database with established conventions (snowflakes as text, timestamptz, idempotent startup schema, graceful degradation), so the event log costs one façade, not one system.

**Why Postgres and not ClickHouse for events.** ClickHouse exists as a foundation store (it backs Langfuse), but the bot has no grant, no egress rule, and no client for it, and friends-scale event volume (thousands of rows per month, not millions per hour) is nowhere near the regime where an OLAP store earns its operational surface. If volume ever demands it, the event log's append-only shape ETLs trivially.

**Why OTLP and not Langfuse's ingestion API.** The deployed Langfuse is v4 (image tag 4.1.0, verified). On v4's default write mode, the legacy batch-ingestion endpoint rejects trace/span/generation events outright — only scores and SDK logs still pass. The OTLP endpoint (`/api/public/otel/v1/traces`, HTTP-only, Basic auth) is the vendor's stated path for languages without an official SDK. This removes the transport decision entirely: OTLP for traces, REST for scores.

**Why the vanilla OpenTelemetry crates and explicit span construction.** No official Rust Langfuse SDK exists, and every community crate is young, low-traction, single-maintainer, or stale — per this repo's pinning policy, none is adoptable. The core `opentelemetry` / `opentelemetry-otlp` crates are actively maintained by the OTel Rust SIG and are a normal pin. The recorder builds spans *explicitly* at the session loop's seams rather than bridging through `tracing` macros: the codebase has zero `tracing` spans today (its `tracing` usage is flat log events under strict level rules), Langfuse needs vendor-specific attributes stamped per-span (no inheritance), and the loop already exposes injection seams (the closures and event sink the shells provide) that make an explicit recorder a natural extension rather than a cross-cutting rewrite. Keeping telemetry out of the `tracing` pipeline also means log filtering and export can never interfere with each other.

**Why capture at the session loop rather than per-tool, and what that costs.** There is exactly one point through which every surface's tool calls and results flow. Instrumenting 26 tool executors individually invites drift (a new tool silently untraced); instrumenting the loop makes coverage structural. The honest price: the loop's observation contract changes — it accepts the recorder explicitly, and its completion hook returns the widened completion result — so its signature and existing tests move with it. That is the deliberate trade: a shell wired into the loop cannot opt out of telemetry by forgetting a wrapper, because the loop itself does the recording. The operator harness's direct-dispatch path bypasses the loop, which is fine — those invocations are deliberately untraced.

**Why scores via REST when traces go via OTLP.** OTLP has no score envelope; the scores REST endpoint is stable, unaffected by the v4 ingestion deprecation, and referenced by trace id — which the recorder knows because it mints trace ids itself before any span is created, rather than reading them back from the exporter. Two small clients, each on the vendor's supported path, beat one client on a deprecated path.

**Why cost lives in Langfuse, not in code.** Prices change and models get swapped (the assessment's biggest cost lever is model routing). Hardcoding a price table in Rust means a deploy per price change and silent staleness; Langfuse's model definitions exist for precisely this and produce cost analytics for free.

**Why the supervisor is untouched.** Giving game pods a telemetry egress or a Postgres grant widens the blast radius the guardrails exist to bound, and supervisor changes ship via manual game-image pushes rather than the gated CI path. Everything this design needs from inside the pod (status, occupancy, restart counts) is already exposed over the control API the bot polls. The in-pod auto-updater's richer sampling stays an untapped source until a real need appears.

**Why occupancy sampling is bot-side polling.** It reuses the exact machinery the deferred-task watcher already trusts, requires no new privileges, and a minutes-cadence sample is enough for playtime, peaks, and packing curves. Push-from-supervisor is more accurate and more surface; not warranted.

**Why the in-cluster egress path to Langfuse.** The public hostname resolves through a Cloudflare proxy and hairpins through the VPS edge — so the "no NetworkPolicy change" path sends every trace (user messages, config file contents) through a third party and couples telemetry to the edge being up. A scoped egress carve-out to the Langfuse namespace matches the four existing bot egress precedents (Postgres, S3, Valkey, supervisor), keeps traffic inside the lab, and lives in this repo where the gate vets it. The carve-out targets the bot pod only, not game pods.

**Why user ids go into Langfuse un-redacted.** The no-identifiers rule protects the *git repo*; Langfuse is lab-hosted infrastructure behind Authentik, the same trust domain as the Postgres database that already stores these ids. Traces necessarily contain user prompt text and file contents — that is what makes them a corpus. The multi-tenant future (strangers, B2B) will need a data-handling policy and possibly redaction; that is a product decision to make when strangers arrive, and it layers onto this design without changing its shape.

**Why 100% capture, no sampling.** The trace corpus is a stated product asset and traffic is tiny; sampling would save nothing and cost corpus. Revisit only if volume grows by orders of magnitude.

**Why Langfuse prompt management is *not* used.** prompt-lib is this repo's source of truth for model-visible text, with its own maintenance contract, and the release tag — the image digest — identifies the exact prompt set by content. Mirroring prompts into Langfuse would create a second authority that drifts. Trace-to-prompt correlation is by release version.

## External touchpoints

**Langfuse OTLP ingestion.** `POST {host}/api/public/otel/v1/traces`, OTLP over HTTP (protobuf or JSON; gRPC unsupported). Auth: HTTP Basic, base64 of `public_key:secret_key`. Header `x-langfuse-ingestion-version: 4` for real-time visibility on v4. Langfuse's attribute namespace (`langfuse.*` — user id, session id, environment, observation type, usage details, tags, metadata, release) takes precedence over OTel GenAI semantic conventions; a span carrying a model attribute is auto-classified as a generation. Trace-level attributes must be stamped on every span. Failure mode: any non-2xx is drop-and-warn; ingestion is at-least-once and Langfuse dedups by span id. The endpoint's availability was verified against the deployed instance.

**Langfuse scores API.** `POST {host}/api/public/scores`, same Basic auth. Payload: trace id, score name, value (number for numeric, string for categorical), data type. Idempotent upsert by optional client-supplied id. Stable API, unaffected by v4 ingestion changes.

**Langfuse UI (manual, one-time per environment).** Project creation, API key minting, model-price definition for the active model names, and saved views/dashboards. Recorded in the operations doc; keys land in 1Password.

**Foundation Postgres.** Two new tables plus a known-guilds set in the bot's existing database and role, following the established conventions: idempotent `CREATE TABLE IF NOT EXISTS` at startup, snowflakes as `TEXT`, `TIMESTAMPTZ DEFAULT now()`, a small dedicated pool, and a façade that degrades to disabled when the database is unreachable. No new database, role, or grant.

**1Password → ExternalSecret → Deployment env.** A new app-scoped secret item carrying the Langfuse key pair, synced by a new ExternalSecret following the existing app-secret naming pattern, mounted as optional env vars so a missing secret means "telemetry off", never a crash loop. Host and non-secret settings ride in the chart's values. Env names follow the app-specific prefix convention.

**Cilium guardrails.** One new egress carve-out in this repo's cluster guardrails: bot pod → Langfuse namespace ingestion port. This is a genuinely new selector shape — the four existing bot carve-outs target either a fixed foundation-host IP or same-namespace pods, while Langfuse needs a cross-namespace pod selector — so it follows the existing files' conventions (bot-only pod selector, sync comment) but not their selector shape. Without it, in-cluster traffic to Langfuse is silently dropped by the namespace leash — the failure is invisible by design, which is why the carve-out ships in the same phase as the exporter.

**Supervisor control API.** Read-only reuse of the existing status and occupancy routes by the sampler. No contract change, no supervisor deploy.

**OpenTelemetry crates.** `opentelemetry`, `opentelemetry-otlp` (HTTP exporter, async client), pinned per current stable at implementation time following the repo's verify-before-pin rule. No Langfuse-specific crates.

## Integration with existing system

**Turn construction.** The four turn-entry shells (Discord message handler, in-game trigger, deferred-batch runner, operator harness) each construct a delivery-agnostic **turn identity** — origin, guild, channel, actor, access tier, session reference, environment — which is what the recorder consumes. The three Discord-shaped shells already share a turn struct and derive the identity from it; the in-game shell, which has no Discord delivery machinery, constructs the identity directly. One shared identity type, deliberately separate from any shell's delivery concerns: a future surface must fill it in to run a turn at all.

**Session loop.** The loop keeps its control flow — injected completion, dispatch, and progress hooks — but its observation contract widens: it takes the recorder as an explicit collaborator, and its completion hook returns the richer completion result. The recorder observes around the completion call for generations, around dispatch for tool observations, and around the whole loop for the root span. The loop's existing outcome (reply, escalated flag) feeds the root span and the `escalated` score.

**Model client.** The response type widens to carry usage and finish reason; the single call site returns them alongside the message with measured latency. Because the loop's completion hook demands the widened result, no shell can silently drop it.

**Guardrail flow.** The snapshot→apply→verify→rollback machinery already produces a closed set of verdicts on the restart path. The recorder maps that verdict to the `guardrail` score; the tool observations already captured the edit's arguments and the verdict text the model saw. No guardrail behavior changes.

**Lifecycle orchestration.** The provisioning, supervisor-client, and backup orchestration layers gain event emission at action-execution points, with the acting user and surface threaded in from callers (slash handlers, Gary tool executors, the deferred runner, scheduled cycles). Today those callers drop the actor; threading it through is part of this work and also fixes the standing attribution gap.

**Prompt-lib contract.** No model-visible text changes. Telemetry reads what the model already sees and says; it adds nothing to prompts, so no prompt files or annotations change *for telemetry's sake* (ordinary annotation maintenance still applies if instrumentation renames functions named in `used_by` entries).

**Deploy path.** Everything ships through the normal gated bot CI: Rust changes, chart changes, and cluster guardrail changes in the same repo, reconciled by Flux. No game-image pushes, no manual cluster mutation beyond what Flux renders.

**Degradation matrix.** The two planes are independent: no Langfuse credentials → no traces, everything else intact; no database → no events or samples (and no home channels, memory, or archives — the existing behavior), traces intact; neither → the bot behaves exactly as it does today. No failure of either plane may delay or fail a user-visible action.

**What this replaces.** Nothing. Existing tracing logs, operator-harness JSON output, escalation DMs, and Discord embeds all stay as they are. The design adds durable capture beside them; it does not reroute any existing signal.

## Out of scope (explicit)

- **RAM right-sizing measurement** — deferred until Gary can act on it (see Goal).
- **Supervisor-side telemetry** — the in-pod sampler and crash history stay untapped; bot-side polling suffices.
- **Fine-tuning export pipeline** — the corpus accumulates in Langfuse; dataset curation is documented capability, not built automation.
- **Redaction / multi-tenant data policy** — required before strangers arrive; layers onto this design without reshaping it.
- **Langfuse prompt management, evals-as-a-service, and the Langfuse MCP server** — available later; nothing here depends on them.
- **Dashboards beyond the documented queries** — Grafana/Langfuse dashboard polish is follow-up, not done-criteria.

## Open questions

None — all decisions above are resolved for this design. The known operational preconditions (Langfuse project + keys minted, 1Password item created, model prices entered) are captured as phase preconditions rather than open design questions.
