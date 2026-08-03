# Operations: data collection

How to get numbers out of the two data planes this service records into. The *why* lives in [ADR-013](../decisions/013-langfuse-span-construction.md) (Langfuse span construction) and [ADR-014](../decisions/014-product-event-log.md) (the product event log); this document is the *how*.

It exists so the productization experiment's metrics are reproducible by someone who has this page, Langfuse access, and database access — and nothing else.

## Shape

Two planes, matched to two shapes of data, both degrading independently to "off".

```
Gary turn ──► Langfuse            (traces, generations, tool calls, scores)
              OTLP + scores REST   → escalation rate, cost per turn

Lifecycle ──► Postgres `events`             (who did what, where, with what outcome)
Sampler   ──► Postgres `occupancy_samples`  (players + restart counter, every 5 min)
                                    → retention, playtime, packing, crash frequency
```

Neither plane can delay or fail a user-visible action. No Langfuse keys means no traces and one log line; no `DB_PASSWORD` means no events and no samples. Both absent and the bot behaves exactly as it did before any of this shipped.

## Quick reference

```bash
just metric escalation-rate          # the headline number
just metric cost-per-turn            # spend per Gary turn
just metric guardrail                # config-edit verdict distribution
just metric volume operator 7        # any metric takes [environment] [days]

just db-query active-users           # weekly active guilds and users
just db-query playtime               # player-hours and server-hours per guild
just db-query peak-concurrency       # the packing input
just db-query crash-frequency        # restart-counter deltas
just db-query acquisition-funnel     # guild_joined → first server
just db-query archive-churn          # archived and never recovered
just db-query capacity-rejections    # creates refused for want of ports

just db-psql                         # interactive shell

just seed-metabase                   # push those queries into Metabase
```

Queries live in [`scripts/queries/`](../../scripts/queries), one file per metric, each carrying its caveats as comments. Read the comments before believing a result.

## Metabase

Every query above is also a saved question at <https://analytics.grizzly-endeavors.com>, in the **Grizzly Gameservers** collection with a **Gameservers** dashboard over them. Use it when the question is exploratory — filter to one guild, drill into a spike, watch a trend across weeks — and `just db-query` when you want the exact documented number.

`just seed-metabase` is what puts them there: it reads `scripts/queries/*.sql`, inlines the two files' psql `\set` variables (Metabase's editor speaks SQL, not psql), carries each file's leading comment block across as the question's description, and rebuilds the dashboard. Re-run it after editing a query — an existing question is updated in place, not duplicated.

**The SQL files stay the source of truth.** Editing a question in Metabase works and will be silently overwritten on the next seed; the dashboard layout is rebuilt from scratch every run. If you want an arrangement that persists, duplicate the dashboard and rearrange the copy.

Metabase reads the database as `metabase_ro`, which holds `SELECT` and nothing else — a native query cannot write to `events` or `occupancy_samples` no matter who runs it. Access is gated to `grizzly-admins` at Authentik. The service itself belongs to grizzly-platform: [ADR-065](https://github.com/Grizzly-Endeavors/grizzly-platform/blob/main/docs/decisions/065-metabase-analytics-service.md) for the why, the [runbook](https://github.com/Grizzly-Endeavors/grizzly-platform/blob/main/docs/runbooks/metabase.md) for operating it.

Langfuse's ClickHouse is connected too, so Gary's traces are queryable beside the event log — but its schema is internal and unversioned, so anything built on it can break on a Langfuse upgrade. The Langfuse read paths below remain the supported way to get those numbers.

## Langfuse

### Project and keys

The service is a platform deployment — see grizzly-platform's [Langfuse runbook](https://github.com/Grizzly-Endeavors/grizzly-platform/blob/main/docs/runbooks/langfuse.md) for the deployment itself. The UI is at <https://langfuse.grizzly-endeavors.com>, behind Authentik.

The key pair is minted per project in the UI under **Settings → API keys**, then flows:

| Stage | Name |
|---|---|
| 1Password item | `gameservers-langfuse`, fields `public_key` / `secret_key` |
| ClusterSecretStore | `onepassword` |
| ExternalSecret + Secret | `grizzly-gameservers-langfuse` |
| Secret keys | `langfuse_public_key`, `langfuse_secret_key` |
| Container env | `LANGFUSE_PUBLIC_KEY`, `LANGFUSE_SECRET_KEY` |

**Rotating the pair takes two actions, not one.** The ExternalSecret uses `refreshPolicy: OnChange`, so it does not poll: update the 1Password item *and* touch the ExternalSecret (annotate it, or `kubectl delete externalsecret` and let Flux recreate it). Updating 1Password alone changes nothing and the bot keeps using the old pair until it restarts.

Both env vars must be present and non-empty or the subsystem disables itself at startup with a single log line: `LANGFUSE_PUBLIC_KEY/SECRET_KEY not set; turn telemetry disabled (everything else still works)`. That is the intended behavior off-cluster, not a fault.

### Host and network path

`LANGFUSE_HOST` defaults to `http://langfuse-web.langfuse.svc.cluster.local:3000` — in-cluster, plain HTTP. Two constraints make that non-negotiable:

- **HTTP, not HTTPS.** The exporter's HTTP client is built without a TLS backend on purpose, so an `https://` host silently fails to export.
- **In-cluster, not the public hostname.** The public name hairpins through Cloudflare and the VPS edge, which would send user messages and config file contents through a third party and couple telemetry to the edge being up.

Egress is allowed by exactly one carve-out, `cluster/guardrails/bot-to-langfuse-egress.yaml`: bot pod → `langfuse`-namespace web pods, TCP 3000 only. If the port in `LANGFUSE_HOST` and the port in that policy ever disagree, traffic is dropped silently — by design, the namespace leash denies rather than rejects. Suspect this first when traces stop arriving with no error in the bot's logs.

### Reading the API

**This deployment runs Langfuse v4 in `events_only` mode, and most of the read API you will find documented online is gone.** Calling a v3 path returns `"This endpoint is not available on deployments running in Langfuse v4 events_only mode"`. Use these:

| Purpose | Path |
|---|---|
| Observations (spans, generations, tool calls) | `GET /api/public/v2/observations?fromStartTime=<iso>&toStartTime=<iso>` |
| Scores | `GET /api/public/v3/scores` |
| Aggregated metrics | `GET /api/public/v2/metrics?query=<url-encoded json>` |
| Model prices | `GET`/`POST /api/public/models` |
| Datasets (corpus curation) | `GET`/`POST /api/public/v2/datasets` |

Dead on this deployment: `/api/public/traces`, `/api/public/observations`, `/api/public/v2/scores`, `/api/public/metrics`.

Two further limits worth knowing before writing a query by hand:

- **There is no `traces` view in the metrics API.** The only views are `observations`, `scores-numeric`, `scores-categorical`, and `scores-boolean`. Anything per-turn is therefore computed from the root `AGENT` observation rather than from a trace row.
- **The `v2/observations` list projection is slim** — it returns no model name, token usage, or input/output. Use the metrics API for those, not the list endpoint. An empty `model` there does not mean the model name is missing.

Filters take an explicit type: `{"column": "environment", "operator": "=", "value": "production", "type": "string"}`.

Authentication is HTTP Basic with the key pair on every one of these. `scripts/langfuse-metric.sh` handles the port-forward, auth, JSON encoding and formatting; reach for `just metric raw '<json>'` when you need a shape it doesn't cover.

### Taxonomies

**Environments.** `production` (real Discord, in-game and deferred traffic), `operator` (the operator harness), `development` (off-cluster runs). The deployment baseline is chosen by whether `KUBERNETES_SERVICE_HOST` is set — a pod is production, a dev box is development — and then narrowed to `operator` for any turn with operator origin. It is not directly configurable, which is what makes harness traffic impossible to accidentally book as product signal.

**Always filter on environment.** Not merely "exclude operator": a handful of early scores were posted before the environment fix landed and sit in Langfuse's `default` bucket. `environment = 'production'` excludes both; `environment != 'operator'` keeps the stale ones.

**Names.** Trace name is `gary.turn.<origin>` — `gary.turn.discord`, `.ingame`, `.deferred`, `.operator`. Within a trace: the root span is `gary.turn` (observation type `AGENT`), each model call is `model.completion` (type `GENERATION`), and each tool call carries the tool's own name (type `TOOL`), so filtering by a real tool name works.

**Scores.** Two, both attached after the turn completes:

| Score | Type | Values |
|---|---|---|
| `escalated` | `BOOLEAN` | every turn that completes; `1`/`0` on the wire |
| `guardrail` | `CATEGORICAL` | only on turns whose restart carried a pending config change: `healthy`, `rolled_back`, `rollback_failed`, `inconclusive`, `unverified` |

A turn that dies on a hard model-endpoint failure exports its trace but **no scores**. Escalation rate is therefore a rate over turns that finished, and provider outages don't silently count as successes.

**Identity.** Every span carries the same set, because Langfuse does not inherit trace attributes: environment, user id (Discord snowflake, or `ingame:<player>`), session id, release, and tags/metadata for origin, access tier, guild and channel. Session id is omitted for deferred batches — each batch is a one-shot conversation with no transcript to belong to.

**Release** is the gate-signed image digest the chart pins (`GAMESERVERS_RELEASE`, `sha256:…`), falling back to `dev` off-cluster. Because prompt-lib compiles all model-visible text into the binary, the digest also identifies the exact prompt set — trace-to-prompt correlation is by release, which is why Langfuse's own prompt management is deliberately unused.

### Model prices

Langfuse holds the price table; the bot never computes money. It ships token counts and the model name, and Langfuse multiplies.

The active model is `glm-5.2` — the literal string the provider echoes back, which is what the price must match. The current definition:

```bash
curl -u "$PUBLIC_KEY:$SECRET_KEY" -X POST "$LANGFUSE_HOST/api/public/models" \
  -H 'Content-Type: application/json' \
  -d '{"modelName":"glm-5.2","matchPattern":"(?i)^(glm-5\\.2)$",
       "unit":"TOKENS","inputPrice":0.00000140,"outputPrice":0.00000440}'
```

$1.40/M input, $4.40/M output. Three things about that payload are easy to get wrong:

- **`unit` is required**, and the write API takes **flat** `inputPrice`/`outputPrice`. The nested `prices` object you see on `GET` is a read-side projection; posting it returns `Must provide either flat prices ... OR pricingTiers`.
- **Do not price `total`.** The bot reports `input`, `output` *and* `total` in usage details; a `totalPrice` would be added on top of the other two and roughly double every figure.
- **Cached input is not broken out.** The provider prices cached input lower ($0.26/M) but the bot doesn't report cached tokens separately, so all input bills at the full rate. Cost figures are therefore a slight over-estimate, not an exact invoice.

**Pricing is applied at ingestion, not at query time.** Defining or changing a price does not reprice observations that already landed — they keep whatever cost they were ingested with (zero, if no price existed). Expect a clean before/after boundary in the data rather than a retroactive correction, and read a `$0.0000` cost on old traces as "ingested before pricing", not "free".

To swap models: enter the new price in Langfuse (UI or the call above), and change `OLLAMA_MODEL`. No deploy is needed for the price, and no code change is needed for either.

## Database

### Connecting

Foundation Postgres is a host-network container on the R730xd at `10.0.0.200:5432`, reached over the flat LAN — not a Kubernetes Service. The bot's image ships no `psql`, so `kubectl exec` into the bot pod is not the way in; connect from the dev box.

```bash
just db-psql -c '\conninfo'
```

`scripts/db-psql.sh` resolves the password from the repo-root `.env` if `DB_PASSWORD` is set there, and otherwise from the ESO-synced cluster secret. The credential's home is 1Password: item `stores-grizzly-gameservers`, field `db_password`, vault `grizzly-platform`, reaching the pod as ExternalSecret `grizzly-gameservers-db`.

Defaults mirror the bot's own config (`crates/bot/src/config.rs`): host `10.0.0.200`, port `5432`, database and role both `grizzly_gameservers`. The instance is shared with other services and capped at 100 connections; don't leave idle sessions open.

### Schema

Three tables, created idempotently at startup. There is no migration tool.

**`events`** — one row per product-meaningful action, written where the action *executes*, not where it is requested, so slash commands, Gary tools, deferred batches, the harness and automatic cycles all land in one stream.

`id`, `kind`, `guild_id`, `game`, `instance`, `actor`, `surface`, `outcome`, `details` (JSONB), `created_at`. Note the column is `kind`, not `type`, and that only `guild_id` carries the `_id` suffix.

The 13 kinds: `server_created`, `server_started`, `server_stopped`, `server_restarted`, `server_shutdown` (GameServer deleted, PVC kept), `server_destroyed` (full teardown), `server_archived`, `server_recovered`, `backup_taken`, `server_restored`, `backup_cycle`, `escalation`, `guild_joined`.

The 5 surfaces: `command` (slash handler), `agent` (a tool in a production Gary turn), `deferred` (a tool in a deferred batch), `operator` (anything through the harness), `system` (automatic cycles).

`actor` is never null: a bare Discord snowflake, `player:<name>` for in-game, or the literal `system`. The prefix exists so a player named `12345` isn't mistaken for a snowflake and one named `system` isn't mistaken for the marker.

`details` payloads by kind: `server_created` and cold `server_started` carry `address` and `time_to_ready_ms`; `backup_taken` and `server_archived` carry `size_bytes`; `server_restored` carries `boot` and `safety_backup`; `server_recovered` carries `address` and `boot`; `backup_cycle` carries `servers`/`succeeded`/`failed`; `escalation` carries `server` and `path`. Everything else is `{}`.

**`known_guilds`** — `guild_id`, `first_seen_at`. Backs `guild_joined` so gateway reconnects, which replay guild events, don't re-fire it. Guilds already connected when the event log first shipped were seeded here *without* events, so the acquisition timeline starts honestly at deployment instead of backdating every existing guild as a join.

**`occupancy_samples`** — `id`, `instance`, `game`, `guild_id`, `players`, `restarts`, `sampled_at`. Appended every `GAMESERVERS_SAMPLE_INTERVAL_MINUTES` (default 5) for each *running* server.

## The metrics

### Escalation rate

`just metric escalation-rate` — the average of the `escalated` score over `production`, which is the experiment's headline number: how often Gary gives up rather than fixing the thing. The assessment's stop signal is >30% after tuning; its go-hard signal is <10%.

`just metric guardrail` gives the companion picture for config edits — how often a restart came back healthy versus rolled back versus worse.

### Cost per turn

`just metric cost-per-turn` — turns (root `AGENT` observations), model calls, tokens, and spend, with the per-turn division done for you. Filter to `production` for the real COGS number; the harness is free in the sense that matters (it's not a customer) but it is not free in tokens, and it is a large share of traffic during development.

A turn is currently ~2 model calls and ~12k tokens, dominated by input — the conversation-so-far is resent on every round. That makes input-token volume, not output length, the cost lever, which is what makes model routing the assessment's biggest lever.

### Retention and engagement

SQL, via `just db-query <name>`:

- **`active-users`** — distinct guilds and users acting per week, harness and automatic actions excluded.
- **`playtime`** — player-hours *and* server-hours per guild per week. The pair matters: servers up all week with zero player-hours is the shape of churn about to happen, and a player-hours-only report renders it as a blank line.
- **`peak-concurrency`** — peak and mean concurrent players per guild per day. This is the packing input the assessment flags as unmeasured, and every margin figure in it rests on this ratio.
- **`acquisition-funnel`** — time from `guild_joined` to the guild's first real server, with unconverted guilds listed.
- **`archive-churn`** — archived and never recovered past a grace window; the closest thing to a silent cancellation this product has.
- **`capacity-rejections`** — creates refused because the port range was full. Recorded separately so a capacity ceiling is never misread as disinterest.

## Reading the data honestly

The traps, each one a real property of the shipped schema. They are repeated in the relevant `.sql` file's comments.

**Never `COALESCE(players, 0)`.** A null player count means *unknown* — a game with no working occupancy probe, or a console unreachable that tick — and never zero. Zero-filling manufactures idle time that did not happen. The flip side is that a game whose probe never works contributes no playtime rather than a wrong number.

**Absence is how "stopped" reads.** Paused instances are not sampled at all, so a stopped server produces no rows, not a run of zeros. A failed *status* read also produces no row (without the restart counter there is no sample); a failed *occupancy* read keeps the row with `players` null.

**Samples are not a fixed grid.** The first tick after startup is skipped so a redeploy doesn't sample before the cluster settles, and missed ticks are delayed rather than dropped — so the cadence is a floor, not a guarantee. Anything integrating over time must use the actual gap between successive samples, clamped, rather than multiplying by five minutes.

**`restarts` is cumulative, not a delta.** Crashes are the positive differences between successive samples of one instance. A *decrease* means the pod was replaced and the counter reset — a boundary, not a negative count. Crashes between a pod's last sample and its replacement are invisible; `crash-frequency` reports the boundary count so the undercount has a visible size.

**Separate harness traffic on `surface`, never on actor.** The operator harness attributes its actions to a real Discord snowflake, so an actor-based filter keeps every one of them.

**Success outcomes are per-kind.** There is no shared `ok`. A successful create is `ready` *or* `unconfirmed` — provisioned but readiness never resolved still means the server exists. `provisioned` and `starting` are near-dead for the normal paths, which hand off to the readiness watcher and record its terminal outcome instead.

**`details` keys are present even when their value is JSON null.** Use `details->>'key' IS NULL`, not `details ? 'key'` — the two disagree here.

**`archives` is append-only.** Query it with `DISTINCT ON (guild_id, name)` newest-first, as the bot's own reads do.

## Corpus curation

The trace corpus is a stated product asset: the full record of each fix — symptom, files read, edit made, outcome — which is simultaneously the fine-tuning dataset, the memory-system evidence base, and B2B sales collateral. It cannot be reconstructed retroactively, which is why capture is 100% with no sampling.

Turning good traces into a dataset uses Langfuse's datasets API, live on this deployment at `/api/public/v2/datasets` (`POST` to create a dataset, then add items referencing a trace or observation id). A tool observation is what makes a trace a fix record: for a config edit its arguments hold the exact old and new text, and its result holds the guardrail verdict the model saw.

**The capability is documented; the pipeline is not built.** Curation is manual today, and automating it is deliberately out of scope.

## Current data coverage

Both planes are live and correct, but young — they began recording on 2026-08-03, and most traffic so far has been operator-harness verification rather than real use. Consequently:

- Escalation rate, cost per turn, playtime, peak concurrency and crash frequency return real values now.
- `active-users`, `acquisition-funnel` and `archive-churn` execute correctly and return empty, because no product-surface (`command`/`agent`) events exist yet. Empty is the honest answer, not a broken query.
- Guild rows seeded at first startup show a null `joined_at` and should be read as pre-existing rather than as instant conversions.

## Common failure modes

**Traces stop arriving, no error in the bot's logs.** Almost always the egress carve-out versus `LANGFUSE_HOST` port mismatch described above — the namespace leash drops silently. Check the policy before suspecting the exporter.

**Cost is $0 with non-zero tokens.** Either no price matches the model name, or those observations were ingested before the price existed. Pricing is not retroactive. `just metric cost-per-turn` says which.

**A metrics query returns "not available ... events_only mode".** A v3 path; see the read-API table.

**Scores appear under environment `default`.** Historical rows from before the environment fix. Filter on `environment = 'production'` rather than excluding `operator`.

**No events or samples at all, bot otherwise healthy.** No `DB_PASSWORD` — the store is disabled and the sampler never starts, both by design, each logging once at startup.

## Dependencies

Foundation Postgres (`10.0.0.200:5432`) and Langfuse (`langfuse` namespace, in-cluster) — both external to this repo, both provisioned in grizzly-platform. This repo owns the two egress carve-outs in `cluster/guardrails/`, the ExternalSecrets and env plumbing in `deploy/`, and the recording code in `crates/bot/`.
