# ADR-013 — Build OTLP spans as `SpanData`, and take a second `reqwest` for the exporter

**Status:** Accepted (2026-08-02)

## Context

`docs/design/data-collection/design.md` settled the transport questions: traces go to Langfuse over OTLP/HTTP (v4's default write mode rejects trace events on the legacy ingestion endpoint), scores go over the REST API, and the client is the vendor-maintained `opentelemetry` / `opentelemetry-otlp` crates rather than any community Langfuse crate. What it could not settle — because it depends on the shape of a specific crate version — is *how* a span gets built, and what that costs the dependency tree.

Two constraints from the bot make this non-obvious. The recorder must know a turn's trace id **before any span exists**, because the scores API attaches to a trace by id. And a turn's generation and tool observations must name the root span as their parent while the root is still running, since the root only ends when the turn does.

## Decision

**Spans are constructed as `opentelemetry_sdk::trace::SpanData` literals and handed to a `BatchSpanProcessor` via `SpanProcessor::on_end`. No `Tracer`, no `SdkTracerProvider`.**

`opentelemetry` 0.32 removed `SpanBuilder::with_trace_id`, `with_span_id`, `with_end_time`, and `with_status`. The span-id removal is decisive: on the `Tracer` path the SDK's `IdGenerator` always assigns the span id, so a child can never be told its parent's id ahead of time, and the root would carry a parent id pointing at nothing. `SpanData` has all-public fields and no `#[non_exhaustive]`, so building it directly is a supported use of the API rather than a workaround — and it takes the sampler and the id generator out of the path entirely, so nothing can override what the recorder minted.

This also buys a property the design assumed away: because the root's span id exists from the start, each observation ships the moment it finishes instead of being buffered until the turn ends. A turn cut short by a redeploy still leaves its model and tool calls in Langfuse.

The cost is that this code owns two invariants the SDK would otherwise own — `TraceFlags::SAMPLED` must be set explicitly (an unsampled span is discarded downstream), and `SpanId::INVALID` is what marks a root. Both are asserted in `telemetry/tests/span.rs` and `agent/tests/recorder.rs`.

**`opentelemetry-otlp` is pinned with `reqwest-blocking-client`, accepting a second `reqwest` major in the tree.**

The default `BatchSpanProcessor` runs on a dedicated std thread and, per its own documentation, does not support async HTTP clients. The blocking client is the supported pairing, not a preference. `opentelemetry-otlp` 0.32 requires `reqwest ^0.13` while the bot is on 0.12, so both end up in the binary.

That is tolerable here for a specific reason: `opentelemetry-http` declares its `reqwest` dependency with `default-features = false` and its blocking feature adds no TLS feature, so **the second copy pulls no TLS backend at all**. The hand-curated native-tls/rustls split in `crates/bot/Cargo.toml` — which exists to keep serenity off an unmaintained rustls chain — is untouched. In-cluster Langfuse is plain HTTP anyway, the same as Postgres, Valkey, and S3. `deny.toml` sets `multiple-versions = "warn"`, so the duplicate is visible without being fatal.

The alternatives were worse. Implementing `opentelemetry_http::HttpClient` over the existing 0.12 client avoids the duplicate but means blocking-bridging an async client from the exporter thread — precisely the reactor hazard the upstream docs warn about. Hand-rolling the OTLP payload avoids both but trades a maintained vendor client for a few hundred lines of protocol code, and reverses the design's stated choice.

## Consequences

- The recorder's five methods stay synchronous `&self`, which is what lets the session loop hold a `&TurnRecorder` across awaits. Per-turn state is atomics and `OnceLock`, never a `Mutex`.
- A turn that dies on the model endpoint still exports an errored root with its partial observations, and no outcome scores — a provider outage is not an agent result.
- The dependency tree carries a second `reqwest` + `hyper` (no TLS, blocking only). Revisit when `opentelemetry-otlp` and the bot converge on one `reqwest` major.
- Local opt-in against Langfuse's **public** hostname does not work: the exporter has no TLS. Reaching it from a dev box means a port-forward to the in-cluster Service. Adding a TLS feature is possible but would put a second TLS chain into the curated set, so it is deliberately not done.
- Because export bypasses the tracer provider, the exported resource has to be set on the processor directly. Without it every span would ship with an empty resource.
- The deployed Langfuse runs in **events-only mode**, so the v3 read endpoints (`/api/public/traces`, `/observations`, `/scores`) return a "not available" error and the legacy `traces`/`observations` ClickHouse tables stay empty by design. Reads go to `GET /api/public/v2/observations?fromStartTime=&toStartTime=` and `GET /api/public/v3/scores`. This affects verification and Phase 5's queries only — the write path (OTLP for spans, `POST /api/public/scores` for scores) is unchanged.
- A score must carry the same `environment` as its trace. Langfuse files an unset one under `default`, which would put operator-harness and production scores in one bucket — and escalation rate is defined as a score aggregation filtered to `production`.
- The `internal-logs` feature is enabled on all three crates deliberately, not by default. Export happens on the SDK's own thread, so a rejected or unreachable endpoint never surfaces through this code's error paths — the subsystem logs "enabled" at startup and then silently drops everything. `internal-logs` routes the SDK's diagnostics through `tracing`, which is what makes that failure visible at the house's log levels.
