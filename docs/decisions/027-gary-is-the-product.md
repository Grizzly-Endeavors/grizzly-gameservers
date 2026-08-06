# ADR-027 — Gary is the product; the game-server platform is his substrate

**Status:** Accepted (2026-08-06)

## Context

This repo was designed from the outside in. The starting problem was "friends want game servers and somebody has to run them," and the ops agent entered the design as a means to an end: each game stores its config differently, and hand-rolling a per-game adapter for every game was the maintenance pile worth avoiding. An LLM that reads whatever layout it finds absorbs that idiosyncrasy generically. That reasoning is still correct, and it is how [`docs/design/00-overview.md`](../design/00-overview.md) came to introduce a Discord shim, then Agones, then the agent.

What that ordering produced was a set of documents describing a game-server platform that happens to include an ops agent. The build went the other way. The overwhelming majority of the work — the guardrail stack, the typed supervisor control API, the escalation exit, snapshot/verify/rollback, cross-guild memory, the deferred-task queue, turn telemetry, the tiered authority model — exists to make an agent safe to leave unattended. None of it is what a hosting product needs. Agones, the catalog, the edge port range, and the PVC lifecycle are, by comparison, undifferentiated: they are the part that gives Gary a real production system to operate.

[`docs/product/productization-assessment.md`](../product/productization-assessment.md) reached the same conclusion from the market side and stated it plainly — the hosting market is commodity, price-driven and low-ARPU, while the thing being proven, an LLM agent that safely mutates production state, is worth considerably more than the market it is being proven in. The assessment recommended pursuing it. That recommendation has been taken.

So the documents and the artifact disagreed about what this is, and the documents were the ones that were wrong. Left alone, that misdirects every future session: an agent reading the overview reasonably concludes that a change is worth making if it improves game hosting, when the actual test is whether it makes Gary a better operator.

## Decision

**Gary is the product. The game-server platform is the substrate he operates.**

The stated purpose inverts: this is not a game-server service with an ops agent attached, it is an ops agent with a real production system to run. Game servers are the domain he was given because they are an unusually good proving ground — genuine per-game idiosyncrasy, real users with real consequences, and a blast radius that is a game world rather than a payments ledger.

**`grizzly-gameservers` remains the project; Gary is the product.** The repo, the Cargo crates, the Kubernetes namespace, the image repository, the Postgres role, the 1Password paths, and the Flux registration in `grizzly-platform` all keep their names. A rename buys a naming consistency that nothing structural depends on, and the infra-level half of it would be a live migration of the namespace holding real users' worlds — real risk against a cosmetic benefit. Identifiers name the project that houses the work; prose names the product.

**Every document outside this decisions record reads as though this were always true.** Per the repo's current-state rule, none of them narrate the shift, mark what changed, or reference an earlier framing. This ADR is the only place the reordering is recorded, which is exactly what the decisions record is for.

**Model-visible prompt text is untouched.** Gary's personas already describe him as the operator, so the reframe creates no contradiction there, and prompt bodies are the human's tuning surface — a wording change is a live behavior change on a deployed bot with real users, and it is not one this decision requires.

## Consequences

- The test a proposed change has to pass changes. "Does this make Gary a more capable or safer operator" is the question; "does this make game hosting better" is a proxy that will sometimes point the other way — most sharply on anything that would buy hosting efficiency by widening what the agent may do unsupervised.
- The guardrails stop reading as overhead on a hosting product and start reading as the product's core claim. Snapshot→apply→verify→auto-rollback, the escalation exit, and the tiered authority model are what "an AI you can leave unattended" means; they are the differentiator, not a tax on it.
- Cross-guild memory is reframed from a convenience to the compounding asset. A quirk learned on one guild's server improving every other guild's is the one property a new entrant cannot start with.
- The substrate is explicitly replaceable. Agones, the catalog and the edge are how Gary reaches *these* servers; the supervisor's typed control API is the contract he actually depends on, and a second backend behind it is a connector rather than a rewrite. That is what keeps the B2B path in `productization-assessment.md` open.
- Docs and identifiers now disagree on purpose. A session reading `grizzly-gameservers` in every path has to take the prose over the paths, which is why the split is stated in the README, in `CLAUDE.md`, and here rather than left to be inferred.
- Nothing deployed changes. This is a framing decision with no code, chart, or cluster component, which is what makes it cheap to take now and expensive to defer.

## Alternatives rejected

- **Rename the project to match the product.** The honest version of the reframe, and it removes the docs-vs-paths split above. Rejected on cost: the prose-level benefit is real but small, while the infra-level half means migrating the namespace that holds live worlds, re-pathing ESO secrets, and reconciling a new Flux registration — a migration with user-visible risk in exchange for consistency of naming.
- **Change the product docs only.** `what-this-is.md` already led with Gary, so this would have been nearly free. Rejected because the documents that actually steer future sessions are `CLAUDE.md` and `docs/design/00-overview.md`, and those were the ones carrying the inverted framing.
- **Leave the framing alone and rely on this ADR.** ADRs are read when someone goes looking for a rationale; the overview is read at the start of every session. A decision that only lives in the decisions record does not reach the sessions it is meant to steer.
- **Reframe Gary's personas and tool descriptions too.** Would carry the framing all the way down to what the model sees. Rejected as out of scope and disproportionately risky: persona wording is the highest-leverage text in the system, an edit lands untested on live users, and the personas are already operator-first.
