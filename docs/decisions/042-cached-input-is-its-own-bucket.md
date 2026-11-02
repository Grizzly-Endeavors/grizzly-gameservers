# ADR-042 — Cached input is its own usage bucket, and `input` carries the remainder

**Status:** Accepted (2026-09-20)

## Context

Gary's turn telemetry ships token counts to Langfuse and lets Langfuse multiply them by a per-model price ([ADR-013](013-langfuse-span-construction.md)). Until now it shipped three counts — `input`, `output`, `total` — straight off the provider's `usage` block, and the operations doc recorded the consequence in a single line: cached input is not broken out, so cost figures are a slight over-estimate.

That line understated the problem by an order of magnitude. Ollama Cloud reports a cache-hit count in `usage.prompt_tokens_details.cached_tokens`, and on `glm-5.2` a repeated prompt comes back with essentially all of it cached — 4025 of 4026 tokens on the second identical call. Gary's loop re-sends the whole transcript on every round, which is the exact shape that hits a prefix cache. Cached input bills at $0.26/M against $1.40/M, so "slight over-estimate" was in practice a headline cost number inflated several-fold, on the one metric the productization question turns on.

The provider's `prompt_tokens` is cache-*inclusive*, and Langfuse computes cost as a plain sum over every usage key it has a price for — no subtraction anywhere. Both halves were verified against the deployed instance rather than taken from the docs: a synthetic generation carrying `{input: 1000, input_cached_tokens: 800, output: 100}` against a probe model priced at $1/M, $0.10/M and $10/M came back at exactly $0.00208, which is additive and not net of the overlap.

## Decision

**The bot makes the buckets disjoint before they leave the process: `input` carries `prompt_tokens - cached_tokens`, and `input_cached_tokens` carries the cache hits.**

Langfuse's own first-party OpenAI integration does this subtraction on the way in, but that is a convenience of that one integration's parser. A flat `usage_details` object — which is what the OTLP span attribute carries — is documented as stored unchanged and required to be exclusive already. Emitting the raw cache-inclusive `prompt_tokens` as `input` alongside a cache bucket would bill every cache hit at both rates at once, which is a larger error in the opposite direction than the one being fixed.

`total` is still shipped and still unpriced. It is what pins Langfuse's token count to the provider's own figure rather than to the sum of the buckets, and pricing it would double every cost.

**A reported zero is shipped; an absent detail block is not.** A provider that says `cached_tokens: 0` has measured a cache miss, and the bucket ships so the turn reads as a genuine miss. A provider that sends no detail block has said nothing, and the whole prompt stays in `input` — the same unknown-is-not-zero rule the other counts already follow.

**`prompt_tokens` stays whole everywhere else.** The harness reports it as the prompt's real size with the cache split beside it, and transcript trimming ([ADR-036](036-context-is-sized-against-the-window.md)) keeps sizing against it — what the context window holds does not change because the provider happened to have some of it cached.

## Consequences

- Cost figures step down at the moment this ships, sharply on `glm-5.2`. Pricing is applied at ingestion and is not retroactive, so the break is a boundary in the data, not a correction of it — a before/after comparison of cost per turn across that boundary is comparing two different accounting rules.
- Each model's Langfuse definition needs an `input_cached_tokens` price or the cache hits price at zero, silently. That price lives in `pricingTiers`; the flat `inputPrice`/`outputPrice` fields cannot express it. Both definitions carry it, and [`docs/operations/data-collection.md`](https://github.com/Grizzly-Endeavors/grizzly-gameservers/blob/pre-hackathon/docs/operations/data-collection.md) is where a new model's entry is recorded.
- Cache coverage is per-model and not a constant. `glm-5.2` reports nearly the whole prompt cached from ~4.5k tokens up; `glm-5.3-flash` reports nothing below roughly 9k and then only part of it. Gary's production generations average ~22k input tokens, so both sit above the threshold on real traffic — but a short conversation on flash legitimately prices at the full rate, and that is the provider's own accounting rather than a gap in ours. The measurements are in [`docs/operations/data-collection.md`](https://github.com/Grizzly-Endeavors/grizzly-gameservers/blob/pre-hackathon/docs/operations/data-collection.md); re-measure when the model changes rather than carrying a ratio forward.
- Langfuse's generic OpenTelemetry `gen_ai.usage.*` extractor has open upstream bugs around exactly this double-counting. Populating `langfuse.observation.usage_details` explicitly, as the recorder already does, keeps the bot out of that path.
