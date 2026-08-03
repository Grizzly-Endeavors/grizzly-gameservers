# ADR-015 — A market-proxy cost plane: quantities in Postgres, money only at query time

**Status:** Accepted (2026-08-03)

## Context

The two data planes (ADR-013, ADR-014) capture what friends do and what Gary's turns cost in tokens, but nothing about what a running server costs to operate — and the business question the productization assessment (`docs/product/productization-assessment.md`) leaves open is exactly that: whether the service can be priced above cost at scale. Its §6 model rests on a rented-dedicated price per GB of RAM and a peak packing ratio it explicitly names as unmeasured. Homelab hardware has no meaningful marginal cost, so "what does this cost me right now" is not the useful number; "what would this workload cost on hardware you can actually rent" is.

Three sub-decisions needed making: what cost model to record against, where the resource quantities come from, and where money enters the system.

## Decision

**Market-proxy framing only, in USD.** Quantities are priced at what the same workload would cost on rented dedicated hardware — the number that decides sellability at scale. There is no homelab-actual modeling: no power/TDP estimation, no hardware amortization, no edge-VPS line (the VPS serves the whole platform and would be a load balancer in a cloud environment). The rate is derived from current list prices (Hetzner AX, OVH Rise) and recorded with its date and sources wherever it appears; the 2026-08-03 derivation lands at $1.50/GB-month, which also corrects the assessment's ~€1.00/GB figure — stale on the low side after Hetzner's June 2026 repricing.

**RAM carries the whole compute price.** The proxy is a box's monthly price divided by its GB of RAM, so pricing core-hours as well would double-count the same box. CPU quantities are still recorded — for packing and right-sizing analysis — but no rate multiplies them.

**A `resource_samples` table beside `occupancy_samples`, written on the same tick.** One row per *existing* instance per tick: actual whole-pod CPU/memory from metrics-server, declared requests/limits summed off the live pod spec, and provisioned PVC capacity. The universe is the managed PVC list, not the running set, because cost outlives the process — a paused instance still reserves its pod's RAM (full row), a shut-down instance still holds its volume (PVC-only row), and only a destroyed instance disappears from the series. This deliberately differs from occupancy's "paused = no row" invariant; the two tables answer different questions and their inserts are separate statements, so their `sampled_at` values never align exactly.

**Declared figures come off the live pod, not the catalog YAML.** The pod spec is what admission actually applied, and it includes the Agones SDK sidecar's footprint, which the catalog templates never mention. Actuals and declared are both whole-pod sums so the packing ratio compares like with like; the sidecar rides along in both (~30m/64Mi), which is honest — it exists only because the game does.

**The bot stores quantities, never money.** Rates live as psql `\set` variables at the top of each cost query in `scripts/queries/`, inlined into Metabase by the seeding script. This mirrors the boundary the Langfuse plane already holds ("the bot never computes money" — Langfuse owns the token price table), and it makes every rate revision retroactive: history is repriced at read time instead of a wrong rate being baked into rows forever.

**Inference cost joins the dashboard through ClickHouse, accepting the schema risk.** Metabase already reads the Langfuse ClickHouse database, so one seeded question against `langfuse.events_core` puts Gary's actual token spend beside the infra estimate. That table is Langfuse-internal and unversioned (the platform's ADR-065 caveat) — in this v4 `events_only` deployment it, not `observations`, is where generations land — so the question may break on a Langfuse upgrade, visibly, and gets fixed then. The alternative — mirroring token counts into Postgres — would duplicate a plane that already exists.

## Consequences

- Cost queries integrate over the actual gap between samples (clamped `lead()`, the `playtime.sql` technique), never `rows × interval`; a null quantity drops out of the sum, so a degraded tick undercounts rather than fabricates.
- The sampler gains two Kubernetes list calls per tick (pods, pod metrics) plus the PVC list, all namespace-scoped; RBAC widens by exactly one read-only rule (`metrics.k8s.io/pods`, get+list). Without metrics-server the actual columns degrade to null and everything else keeps working.
- `metrics.k8s.io` has no typed bindings in k8s-openapi, so the sampler reads `PodMetrics` through `Api<DynamicObject>` — the same pattern as the Agones `GameServer` access — with the payload deserialized into typed structs so a schema surprise fails loudly.
- The packing ratio — the assessment's declared unmeasured number — becomes a query (`packing-ratio.sql`) instead of an assumption.
- Rate revisions are a query-file edit plus `just seed-metabase`, no deploy. The dated derivation block in each file is the audit trail.
- Storage run-rate for backups reads the event log's recorded `size_bytes` (ADR-004 manifests), not the bucket — cheap, but it trusts the log; a bucket-side audit stays possible through the manifests if the two ever need reconciling.
