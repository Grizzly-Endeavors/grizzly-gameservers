# Data Collection for Productization — Implementation Phases

> **Archived.** All five phases are complete. Kept as the record of how the work was sequenced. For the shipped subsystem, see [`docs/operations/data-collection.md`](../../operations/data-collection.md).
>
> Module level only. No file or line references. Each phase is self-contained, depends only on phases before it, and is verifiable on its own. The companion design is [design.md](design.md).

## Phase 1 — Turn identity and completion widening

- **Modules:** model client, session loop, session store, the four turn-entry shells (Discord, in-game, deferred, operator), operator-harness response types.
- **Preconditions:** none.
- **Shape when done:** Every turn carries the delivery-agnostic turn identity from the design (origin discriminant, guild, channel, actor, access tier, session reference, environment), constructed by all four shells. The model client's response type carries token usage, finish reason, and measured latency as the widened completion result, and the session loop's completion hook returns that type. The session loop accepts a recorder collaborator (a no-op recorder in this phase — the seam exists, nothing exports). The session store mints an opaque per-transcript session id, created with a fresh transcript and retired with it, never derived from the store's keying. The operator harness's turn response additionally exposes per-call token usage and latency, making capture observable before any exporter exists.
- **Verification:** Workspace tests pass, including new unit tests for the identity construction and the widened completion result. After deploy, an operator-harness Gary turn returns usage and latency figures alongside the existing reply/rounds/tool-trace fields, and behaves identically otherwise. All four origins still complete turns (Discord mention, deferred task, operator; in-game if a server is up).

## Phase 2 — Langfuse export

- **Modules:** telemetry subsystem (recorder, OTLP exporter, scores client), bot config loader, Helm chart (values, ExternalSecret, Deployment env — including release-digest injection), cluster guardrails (bot→Langfuse egress carve-out).
- **Preconditions:** Phase 1 (turn identity, widened completion result, recorder seam). Operational: a Langfuse project exists with a minted API key pair, stored in 1Password under the app's item per the platform secrets flow.
- **Shape when done:** The no-op recorder is replaced by the real one: every model-driven turn exports a trace over OTLP (HTTP, Basic auth, v4 ingestion header) with root span, generation observations (messages, model, usage, finish reason, latency), tool observations (name, arguments, result, duration), and the full identity attribute set stamped per span — environment, user id, session id, release (image digest), origin/guild/game/tier tags and metadata. Outcome scores (`escalated` always, `guardrail` when applicable, per the design's closed taxonomies) post via the scores REST API. Export is batched and background, flush joins the existing shutdown drain, absent credentials disable the subsystem with one log line, and export failure logs on state change only. The egress carve-out (cross-namespace selector, per the design) ships in the same change as the exporter.
- **Verification:** Drive a Gary turn through the operator harness; the trace appears in the Langfuse UI under environment `operator` with generations showing real token counts and tool observations showing verbatim arguments and results, and the `escalated` score attached. A config-edit-plus-restart turn produces a `guardrail` score. A real Discord turn lands under `production` with a session id, and a second turn in the same conversation shares it. Removing the credentials (or running where they're absent) yields the single disabled log line and an otherwise fully functional bot.

## Phase 3 — Product event log

- **Modules:** event store façade (events, known-guilds tables), lifecycle orchestration (provision, warm lifecycle, destroy), backup orchestration (backup, archive, restore, recover, scheduled cycle), escalation notifier, gateway guild handler, the callers that thread actor and surface through (slash handlers, Gary tool executors, deferred runner, operator routes).
- **Preconditions:** Phase 1 (the origin/surface vocabulary). Independent of Phase 2.
- **Shape when done:** The append-only events table exists with the design's closed event catalog, written at action-execution points with actor and surface attribution threaded from every caller — closing the standing gap where lifecycle actions record no actor. `guild_joined` fires on first-ever sighting backed by the known-guilds table, seeded without events on first startup. Escalation events record at determination, all origins, with kind-dependent optional fields. The façade follows the established graceful-degradation shape: no database, no events, bot unaffected.
- **Verification:** Unit tests on the pure emission mapping (outcome variant → event row shape). After deploy: create, stop, and destroy a scratch server via the operator harness and observe one row each with correct actor, surface `operator`, game, guild, and outcome; run a manual backup and observe its row; confirm the seeded known-guilds table matches connected guilds and no `guild_joined` rows were backdated. Rows survive bot redeploys; a DB outage degrades to the existing no-persistence behavior.

## Phase 4 — Occupancy sampling

- **Modules:** occupancy sampler (new background task in the bot), event store façade (samples table), server enumeration (surfacing guild alongside instance/game/state).
- **Preconditions:** Phase 3 (the store façade and its degradation shape).
- **Shape when done:** A minutes-cadence sampler enumerates running servers (with guild), makes the two control-API calls per instance (status, occupancy), and appends samples — timestamp, instance, game, guild, player count (null means unknown, never zero), process-restart counter. The sampler starts and stops with the bot's existing background-task lifecycle and degrades to off when the store is disabled.
- **Verification:** With at least one running server, sample rows accumulate at the configured cadence; a game without a working occupancy probe records null players rather than zero; the restart counter matches the supervisor's status route. Stopping the server stops its samples. Sampler logs stay silent at info level per the house log rules.

## Phase 5 — Derivation layer

- **Modules:** documentation only (a data-collection operations doc in this repo), plus manual Langfuse configuration (model-price definitions, saved views) recorded by that doc.
- **Preconditions:** Phases 2–4 deployed and accumulating real data.
- **Shape when done:** An operations doc records: the Langfuse project setup and key rotation path, the environment taxonomy, the score names and their closed value sets, the model-price definition for the active model (entered in Langfuse, so cost-per-turn renders), and the runnable queries for the experiment metrics — escalation rate (score aggregation over `production`), cost per turn (Langfuse cost views), retention/engagement (SQL over events and samples: active guilds/users per week, playtime per guild, `guild_joined`→first-create funnel, archive-without-recover churn) — plus the documented trace→dataset curation path for the future corpus export.
- **Verification:** Someone with only this doc, Langfuse access, and database access can produce each metric: every documented query runs against live data and returns plausible, explainable values; cost figures appear on traces in Langfuse; the escalation-rate view excludes `operator` traffic.
