# ADR-014 — Record product events at the executing caller, in one wide table

**Status:** Accepted (2026-08-02)

## Context

[`docs/archive/data-collection/design.md`](https://github.com/Grizzly-Endeavors/grizzly-gameservers/blob/pre-hackathon/docs/archive/data-collection/design.md) settles *what* the product plane records — a closed event catalog, one append-only table in the bot's existing Postgres, attributed by actor and surface — and *why* it is separate from the Langfuse plane. What it leaves open is where in the code an event is emitted, which turns out to be the load-bearing question.

The design says events are emitted "at the orchestration layers where the action actually executes (not where it is requested)". Read literally, that points at the `agones` functions — `provision_instance`, `shutdown_instance`, `destroy_instance`, `begin_start`, `supervisor_stop`/`start`/`restart` — and instrumenting there would make coverage structural: a new caller could not forget to record. That is the same argument that put turn telemetry in the session loop rather than in 26 tool executors (ADR-013).

Two facts about this codebase defeat that reading.

**Those functions are also internal building blocks.** `archive_instance` tears a server down through the same `destroy_instance` a friend's `/destroy` calls. The backup flows cold-start a shut-down pod through `begin_start` and pause it through `supervisor_stop` to get a consistent snapshot. The stranded reconciler recycles through `begin_start` too. Emitting inside them would put a `server_destroyed` beside every `server_archived` and a `server_started` beside every archive — double-counting exactly the teardowns and starts the churn and engagement queries are built on.

**A create's terminal outcome is not in the create.** `provision_instance` deliberately returns before readiness so the caller can show the friend an address and then poll (`wait_for_instance_ready`). The catalog's rule is that `server_created` fires at its terminal outcome — readiness resolution for a successful provision — and carries time-to-ready. Only the caller awaits that, so only the caller can record it.

## Decision

**Lifecycle events are emitted by the layer that executes the action — the slash handler, the Gary tool executor, the scheduled cycle — with all outcome mapping in one pure module (`events::mapping`).**

Internal composition stays out of the event stream for free, and the readiness-resolving caller is the one that records. The lost structural guarantee is bought back on the axis that actually rots: `wildcard_enum_match_arm` is denied workspace-wide, so every mapper enumerates every variant of every orchestration outcome. A new `ProvisionOutcome` or `SupervisorOutcome` variant breaks the build rather than silently landing as an unmapped row.

**Backups are the exception, and emit inside `BackupService`.** That layer already *is* what every surface shares — slash commands, Gary's tools, and the scheduled cycle all reach the backup flows through the same four methods — so emitting there is emitting at the executing layer, not above it. Each of the four public methods wraps a private `run_*` so its many early returns are covered by one emission point.

**The stranded reconciler's recycle emits `server_started` with surface `system`.** It is a genuine start that nobody asked for, and strand-and-recycle frequency is the only crash signal with no human in the loop. Backup's internal cold-start and safety-stop stay silent — those are implementation details of `backup_taken` and `server_archived`.

**Gary's create and cold start resolve readiness in a tracked background task.** `exec_create` must not block: first-boot world generation runs for minutes and the model would stall mid-turn. So the wait moves off the turn rather than off the record — the tool returns the address immediately as before, and a task on the shared `TaskTracker` records the terminal outcome with real timing once readiness resolves. It races `Data::shutdown`, so a redeploy is never held for the 5-minute readiness budget.

**One wide `events` table, not a table per action.** The catalog is closed and every row answers the same question — who did what, where, and how did it end — so the derivation layer's queries are `WHERE kind = ...` rather than a union across a dozen shapes. `guild_id`, `game`, and `instance` are nullable because not every kind has all three, and event-specific payload goes in a `details` JSONB column. `details` is bound as text and cast `::jsonb`, because sqlx is pinned without its `json` feature and the cast costs nothing.

**`surface` is a new five-variant enum, not a reuse of `TurnOrigin`.** Origin answers "how did a Gary turn arrive"; surface answers "what drove this action". Two surfaces have no turn behind them at all (`command`, `system`), and `agent` deliberately collapses the two production Gary origins. `Surface::from_origin` covers every agent path, so a `run_when` batch and an operator-harness turn are separable with no extra threading — `ToolCtx` already carries the origin.

**The outcome column carries the escalation kind rather than a constant.** `round_budget_exhausted` and `crash_rollback` are separable with `GROUP BY outcome`, no JSONB extraction; the server and file locators go in `details`.

## Consequences

- Actor attribution is closed for every lifecycle action, which it never was: `provision_instance`, `destroy_instance`, `shutdown_instance`, `begin_start`, and the three supervisor calls all dropped it. `backup_instance` and `archive_instance` had a `created_by: &str`; that widens to `&Attribution` (actor *and* surface), and `restore_backup` / `recover_archive` gain the same parameter. The S3 manifest's own `created_by` convention (`auto`, `auto-pre-restore`) is untouched — it stays internal to the backup flows.
- A warm action knows only an instance name, so `EventLog::record_lifecycle` reads the owning guild and game off the instance's Service (`agones::owner_of`) when the caller doesn't already have them. That is one extra API read per lifecycle event, skipped entirely when the log is disabled, and best-effort: a failed lookup costs the row its tenant columns, never the friend their action.
- `archive_instance` resolves the owner *before* running, because a successful archive releases the Service those labels live on.
- `guild_joined` needs the `known_guilds` table because `GuildCreate` re-fires on every gateway reconnect. On `Ready` the set is reconciled from the gateway's own guild list, emitting nothing, so the acquisition timeline starts at deployment rather than backdating every existing guild as a join. Until that reconcile lands, a `GuildCreate` is claimed silently — a genuine join in that sub-second window is recorded as known without an event, which is the honest trade against backdating a whole guild list.
- `EventLog::record` returns nothing and swallows-and-logs, matching `Telemetry::emit`. Emission sites are on user-visible paths and have nothing useful to do with the error, so they never branch on the fact that we happen to be keeping records.
- The escalation event is emitted beside `notifier.notify`, not inside it, at all five determination sites. `OperatorNotifier::notify` returns early when no operator is configured, and the operator harness never notifies at all — an escalation happened either way. The harness gains exactly the event and nothing else; its caller already sees the `escalated` flag.
- No chart or cluster change. The tables live in the bot's existing database and role, and `bot-to-postgres-egress.yaml` already opens the path.
