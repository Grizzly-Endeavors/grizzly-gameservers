# Operations: data collection

How to get numbers out of the two data planes Gary records into — his own turns, and the product usage around them. The *why* lives in [ADR-013](../decisions/013-langfuse-span-construction.md) (Langfuse span construction) and [ADR-014](../decisions/014-product-event-log.md) (the product event log); this document is the *how*.

It exists so the productization experiment's metrics are reproducible by someone who has this page, Langfuse access, and database access — and nothing else.

## Shape

Two planes, matched to two shapes of data, both degrading independently to "off".

```
Gary turn ──► Langfuse            (traces, generations, tool calls, scores)
              OTLP + scores REST   → escalation rate, cost per turn

Lifecycle ──► Postgres `events`             (who did what, where, with what outcome)
Sampler   ──► Postgres `occupancy_samples`  (players + restart counter, every 5 min)
                                    → retention, playtime, packing, crash frequency
Sampler   ──► Postgres `resource_samples`   (pod usage, requests, pvc bytes, every 5 min)
                                    → cost per server, packing ratio
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
just db-query cost-per-server        # market-proxy USD per instance
just db-query cost-monthly-summary   # fleet USD per month, by component
just db-query packing-ratio          # actual over requested; the margin input

just db-psql                         # interactive shell

just seed-metabase                   # push those queries into Metabase
```

Queries live in [`scripts/queries/`](../../scripts/queries), one file per metric, each carrying its caveats as comments. Read the comments before believing a result.

## Metabase

Every query above is also a saved question at <https://analytics.grizzly-endeavors.com>, in the **Grizzly Gameservers** collection with a **Gameservers** dashboard over them. Use it when the question is exploratory — filter to one guild, drill into a spike, watch a trend across weeks — and `just db-query` when you want the exact documented number.

`just seed-metabase` is what puts them there: it reads `scripts/queries/*.sql` (plus `scripts/queries/clickhouse/*.sql`, see below), inlines each file's psql `\set` variables (Metabase's editor speaks SQL, not psql), carries each file's leading comment block across as the question's description, and rebuilds the dashboard. Re-run it after editing a query — an existing question is updated in place, not duplicated.

**The SQL files stay the source of truth.** Editing a question in Metabase works and will be silently overwritten on the next seed; the dashboard layout is rebuilt from scratch every run. If you want an arrangement that persists, duplicate the dashboard and rearrange the copy.

Metabase reads the database as `metabase_ro`, which holds `SELECT` and nothing else — a native query cannot write to `events` or `occupancy_samples` no matter who runs it. Access is gated to `grizzly-admins` at Authentik. The service itself belongs to grizzly-platform: [ADR-065](https://github.com/Grizzly-Endeavors/grizzly-platform/blob/main/docs/decisions/065-metabase-analytics-service.md) for the why, the [runbook](https://github.com/Grizzly-Endeavors/grizzly-platform/blob/main/docs/runbooks/metabase.md) for operating it.

Langfuse's ClickHouse is connected too, so Gary's traces are queryable beside the event log — but its schema is internal and unversioned, so anything built on it can break on a Langfuse upgrade. Exactly one seeded question accepts that risk: **Inference cost**, from `scripts/queries/clickhouse/inference-cost.sql`, which puts Gary's actual token spend on the dashboard beside the infra cost lines (Metabase cannot join across databases, so it is its own card). The subdirectory is what routes a file to the ClickHouse database — `just db-query` never touches it, so psql can't be pointed at ClickHouse SQL. If a Langfuse upgrade breaks the card, re-verify the table and columns (in this v4 `events_only` deployment generations live in `events_core`, not `observations`) and reseed. For everything else, the Langfuse read paths below remain the supported way.

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

**Scores.** Four, all attached after the turn completes:

| Score | Type | Values |
|---|---|---|
| `escalated` | `BOOLEAN` | every turn that completes; `1`/`0` on the wire |
| `escalation_kind` | `CATEGORICAL` | only on turns that escalated: `agent_requested` (Gary called the escalate tool and stopped) or `round_budget_exhausted` (the loop ran out) |
| `cancelled` | `BOOLEAN` | every turn that completes; `1` when somebody ran `/gary-stop` against it |
| `guardrail` | `CATEGORICAL` | only on turns whose restart carried a pending config change: `healthy`, `rolled_back`, `rollback_failed`, `inconclusive`, `unverified` |

`escalated` stays boolean and stays on every turn because the escalation rate is an average over it — `escalation_kind` splits the numerator, never the denominator. Its values are the same strings the `events` table writes into `outcome` for `kind='escalation'`, so the two planes join on them.

`cancelled` is a sibling of `escalated`, never a value inside it. A stopped turn is an operator action, not Gary failing to finish, so counting it as an escalation would put the two in one number and make the headline metric unreadable the moment anyone started using the command ([ADR-037](../decisions/037-stopping-a-running-turn.md)). It is boolean on every turn for the same reason `escalated` is — so its own rate has an honest denominator. A stopped turn still exports the generations it paid for, so it counts toward cost.

A turn that dies on a hard model-endpoint failure exports its trace but **no scores**. Escalation rate is therefore a rate over turns that finished, and provider outages don't silently count as successes.

**Identity.** Every span carries the same set, because Langfuse does not inherit trace attributes: environment, user id (Discord snowflake, or `ingame:<player>`), session id, release, and tags/metadata for origin, access tier, guild, channel and game. Session id is omitted for deferred batches — each batch is a one-shot conversation with no transcript to belong to.

**`metadata.game` is present only where the surface knows a server up front**, so a per-game cost or escalation-rate breakdown covers part of the traffic, not all of it. An in-game trigger and a deferred batch both arrive for a named server and resolve the game off its labels; a Discord turn usually names no server and omits the key entirely rather than guessing. Treat "no game" as *unattributed*, never as a game named `""` — and when you need the full split, join to the `events` table on guild + time rather than assuming the traces carry it.

**Release** is the gate-signed image digest the chart pins (`GAMESERVERS_RELEASE`, `sha256:…`), falling back to `dev` off-cluster. Because prompt-lib compiles all model-visible text into the binary, the digest also identifies the exact prompt set — trace-to-prompt correlation is by release, which is why Langfuse's own prompt management is deliberately unused.

### Model prices

Langfuse holds the price table; the bot never computes money. It ships token counts and the model name, and Langfuse multiplies.

**The price matches the literal string the provider echoes back**, which for Ollama Cloud is the tag that was requested, verbatim — ask for `glm-5.3-flash:cloud` and that is what comes back, so every pattern here tolerates the optional `:cloud` suffix. A tag the pattern misses is not an error; it is a silent $0.

Two definitions are entered, both priced from the model's page on ollama.com:

| Model | Input | Cached input | Output |
|---|---|---|---|
| `glm-5.3-flash` | $0.15/M | $0.03/M | $0.50/M |
| `glm-5.2` | $1.40/M | $0.26/M | $4.40/M |

```bash
curl -u "$PUBLIC_KEY:$SECRET_KEY" -X POST "$LANGFUSE_HOST/api/public/models" \
  -H 'Content-Type: application/json' \
  -d '{"modelName":"glm-5.3-flash","matchPattern":"(?i)^(glm-5\\.3-flash)(:cloud)?$","unit":"TOKENS",
       "pricingTiers":[{"name":"Standard","isDefault":true,"priority":0,"conditions":[],
         "prices":{"input":0.00000015,"output":0.00000050,"input_cached_tokens":0.00000003}}]}'
```

Five things about that payload are easy to get wrong:

- **`unit` is required**, and a price for anything beyond input and output needs **`pricingTiers`**. The flat `inputPrice`/`outputPrice` fields only reach those two, and the nested `prices` object you see on `GET` is a read-side projection — posting it returns `Must provide either flat prices ... OR pricingTiers`.
- **Cost is a plain sum over every usage key that has a matching price**, with no subtraction anywhere. That is why the bot ships `input` as the *uncached* remainder and the cache hits as `input_cached_tokens`: sending the provider's cache-inclusive `prompt_tokens` as `input` would bill each cache hit at both rates at once. The buckets have to be disjoint before they leave the bot ([ADR-042](../decisions/042-cached-input-is-its-own-bucket.md)).
- **Do not price `total`.** The bot reports `input`, `output` *and* `total` in usage details; a `totalPrice` would be added on top of the others and roughly double every figure. Unpriced, `total` is what pins Langfuse's token count to the provider's own figure instead of the sum of the buckets.
- **There is no `PUT`.** Editing a definition means `DELETE /api/public/models/{id}` then posting it again. Costs already ingested are untouched by that, so it is safe — but the new definition is a new row, and its id changes.
- **Pricing is applied at ingestion, not at query time.** Defining or changing a price does not reprice observations that already landed — they keep whatever cost they were ingested with (zero, if no price existed). Expect a clean before/after boundary in the data rather than a retroactive correction, and read a `$0.0000` cost on old traces as "ingested before pricing", not "free".

**Cache hits are a large share of what a Gary turn reads, and the rate gap is roughly 5×**, so whether they are broken out moves the cost figure a lot rather than a little: a turn's transcript is re-sent in full on every round, and `glm-5.2` reports nearly all of it served from cache by the second call. `glm-5.3-flash` reports `cached_tokens: 0` even on an identical repeated prompt, so on that model the split is recorded and prices at the full input rate — which is the honest reading of what the provider says it charged.

To swap models: enter the new price as above, and change the `gary.model` setting (admin panel) or `OLLAMA_MODEL`. No deploy is needed for the price, and no code change is needed for either.

## Database

### Connecting

Foundation Postgres is a host-network container on the R730xd at `10.0.0.200:5432`, reached over the flat LAN — not a Kubernetes Service. The bot's image ships no `psql`, so `kubectl exec` into the bot pod is not the way in; connect from the dev box.

```bash
just db-psql -c '\conninfo'
```

`scripts/db-psql.sh` resolves the password from the repo-root `.env` if `DB_PASSWORD` is set there, and otherwise from the ESO-synced cluster secret. The credential's home is 1Password: item `stores-grizzly-gameservers`, field `db_password`, vault `grizzly-platform`, reaching the pod as ExternalSecret `grizzly-gameservers-db`.

Defaults mirror the bot's own config (`crates/bot/src/config.rs`): host `10.0.0.200`, port `5432`, database and role both `grizzly_gameservers`. The instance is shared with other services and capped at 100 connections; don't leave idle sessions open.

### Schema

Four tables, created idempotently at startup. There is no migration tool.

**`events`** — one row per product-meaningful action, written where the action *executes*, not where it is requested, so slash commands, Gary tools, deferred batches, the harness and automatic cycles all land in one stream.

`id`, `kind`, `guild_id`, `game`, `instance`, `actor`, `surface`, `outcome`, `details` (JSONB), `created_at`. Note the column is `kind`, not `type`, and that only `guild_id` carries the `_id` suffix.

The kinds: `server_created`, `server_started`, `server_stopped`, `server_restarted`, `server_shutdown` (GameServer deleted, PVC kept), `server_destroyed` (full teardown), `server_archived`, `server_recovered`, `archive_deleted`, `backup_taken`, `server_restored`, `backup_cycle`, `escalation`, `guild_joined`, `incident`, `resources_changed`.

The 5 surfaces: `command` (slash handler), `agent` (a tool in a production Gary turn), `deferred` (a tool in a deferred batch), `operator` (anything through the harness), `system` (automatic cycles).

`actor` is never null: a bare Discord snowflake, `player:<name>` for in-game, or the literal `system`. The prefix exists so a player named `12345` isn't mistaken for a snowflake and one named `system` isn't mistaken for the marker.

`details` payloads by kind: `server_created` and cold `server_started` carry `address` and `time_to_ready_ms`; `backup_taken` and `server_archived` carry `size_bytes`; `server_restored` carries `boot` and `safety_backup`; `server_recovered` carries `address` and `boot`; `backup_cycle` carries `servers`/`succeeded`/`failed`; `escalation` carries `server` and `path`. Everything else is `{}`.

A `resources_changed` row's `outcome` says how the resize landed: `applied` (the running container took it, nobody disconnected), `in_progress` (the kubelet accepted it and was still settling), `recorded_for_next_start` (no running pod to change), or `infeasible` (the node declined the live change; the setting still applies at the next start). Paired with `resource_samples`, this is the right-sizing measurement — the samples say what an instance was given per tick, these rows say when somebody decided to change it. See [ADR-021](../decisions/021-in-place-resource-resize.md).

An `escalation` row's `outcome` says which exit the turn took: `agent_requested` (Gary called the escalate tool), `round_budget_exhausted` (the loop ran out of rounds), or `crash_rollback` (an automatic config rollback couldn't recover the server — no turn involved). The first two are the interesting split; see [ADR-019](../decisions/019-agent-invoked-escalation.md).

**`known_guilds`** — `guild_id`, `first_seen_at`. Backs `guild_joined` so gateway reconnects, which replay guild events, don't re-fire it. Guilds already connected when the event log first shipped were seeded here *without* events, so the acquisition timeline starts honestly at deployment instead of backdating every existing guild as a join.

**`occupancy_samples`** — `id`, `instance`, `game`, `guild_id`, `players`, `restarts`, `sampled_at`. Appended every `GAMESERVERS_SAMPLE_INTERVAL_MINUTES` (default 5) for each *running* server.

**`resource_samples`** — `id`, `instance`, `game`, `guild_id`, `cpu_millicores`, `mem_bytes`, `cpu_request_millicores`, `mem_request_bytes`, `cpu_limit_millicores`, `mem_limit_bytes`, `pvc_bytes`, `sampled_at`. Appended on the same interval for each *existing* instance — running, paused, or shut down. Actuals come from metrics-server, declared figures off the live pod spec (whole-pod sums, Agones sidecar included), `pvc_bytes` from the claim. Raw quantities only, never money: the rate card lives in the query files, so a rate change reprices all of history at read time ([ADR-015](../decisions/015-market-proxy-cost-plane.md)).

## The metrics

### Escalation rate

`just metric escalation-rate` — the average of the `escalated` score over `production`, which is the experiment's headline number: how often Gary hands a request to a human rather than fixing the thing. The assessment's stop signal is >30% after tuning; its go-hard signal is <10%.

Read it alongside `cancelled`. A turn somebody stopped never got the chance to escalate, so a period with a lot of stops has an escalation rate measured over fewer real attempts — and a rising stop rate is its own signal, usually that turns are running long enough for people to give up on them.

Split it by `escalation_kind` before reading it as a verdict, because the two exits mean opposite things. `agent_requested` is the guardrail working — Gary judged he couldn't finish and stopped instead of guessing. `round_budget_exhausted` is thrashing: sixteen rounds spent without converging *or* deciding to stop. A rate that holds steady while the mix shifts toward `agent_requested` is an improvement, not a regression.

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

### Operational cost (market proxy)

Also SQL, over `resource_samples`:

- **`cost-per-server`** — market-proxy USD per instance over the window: RAM GB-hours on requests-basis (what capacity has to be provisioned for) and actual-basis (the right-sized floor), core-hours as unpriced quantities, storage GB-months.
- **`cost-monthly-summary`** — fleet USD per month as component lines: RAM both bases, PVC storage, and the current S3 backup/archive footprint as a run-rate line. The inference line is *not* here — it lives in the Langfuse plane (`just metric cost-per-turn`), since Metabase cannot join across databases.
- **`packing-ratio`** — actual over requested, per game and fleet-coincident. The `(fleet, coincident)` row's max is peak coincident packing — the assessment's declared-unmeasured number, and what capacity actually has to be sized for.

The cost model is [ADR-015](../decisions/015-market-proxy-cost-plane.md): quantities in Postgres, money only in the query files' `\set` rate variables, priced at read time in USD. Two rates exist — `ram_usd_gb_month` (1.50) and `storage_usd_gb_month` (0.004) — derived 2026-08-03 from rented-dedicated list prices (Hetzner AX42-1, OVH Rise-S/Rise-Game-2; Hetzner Storage Box BX11 for storage; EUR→USD 1.15). RAM carries the whole compute price because the proxy is a box's price over its GB of RAM — pricing core-hours as well would double-count the box. To revise a rate: edit the `\set` lines and the derivation comment in each cost query, then `just seed-metabase`; no deploy, and history reprices itself.

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

**Paused instances DO produce resource rows — the opposite of occupancy.** A paused pod still reserves its RAM, so `resource_samples` keeps recording it. In that table a full row means "pod exists" (running or paused), a PVC-only row (all compute columns null) means "shut down", and absence means "destroyed". Don't infer running-ness from `resource_samples`; that's what `occupancy_samples` is for.

**Null actuals mean metrics-server had no reading, never "idle".** A young pod, an unreachable metrics API, or missing RBAC all read as null `cpu_millicores`/`mem_bytes` with the declared columns intact. Summing treats them as absent — an undercount, not a zero.

**Resource figures are whole-pod sums including the Agones sidecar.** Both the declared and actual sides count every container, so packing ratios compare like with like — but neither is "just the game". The limit columns sum only the containers that declare one (the sidecar typically declares none), so they are in practice the game container's burst envelope.

**Never join `resource_samples` to `occupancy_samples` on `sampled_at` equality.** The two tables are written by separate statements in the same tick, so their timestamps are close but never identical. Join on time buckets (`date_trunc`) if you must correlate them.

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
