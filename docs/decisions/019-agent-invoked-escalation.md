# ADR-019 — The agent escalates by calling a tool, and that ends the turn

**Status:** Accepted (2026-08-04)

## Context

`docs/design/00-overview.md` names the escalation exit as one of three non-negotiable ops-agent guardrails: *"Give the agent an explicit 'I'm not confident — paging Bear' path so open-ended break-fixing escalates instead of thrashing."* What shipped was the round budget. `run_session` looped up to `DEFAULT_MAX_ROUNDS`, and falling out the bottom set `escalated: true`, returned a fixed sentence, and DMed the operators. The bounded loop was doing real work — it is what stops a runaway — but it is a backstop, not an exit. The agent could not choose it.

The gap showed up as two behaviours. A turn that ran out of ideas after four rounds had nothing to do with the remaining twelve except repeat itself, and the model has no view of its own budget, so it could not tell "I am stuck" from "I have more to try." And a turn that genuinely could not be finished — the fix needs credentials Gary doesn't have, or a judgement about which backup is the right one — produced a confident-sounding answer instead, because nothing in the prompt said stopping was allowed. On live game worlds belonging to real friends, a plausible wrong answer costs more than a handoff.

The escalation vocabulary was also single-valued. `SessionOutcome.escalated` was a bool, the operator DM had exactly one shape for a turn that gave up, and `escalation_details` recorded one outcome string for it. There was no way to ask "how often does Gary stop deliberately versus thrash into the ceiling" — which is the question that says whether the guardrail works.

## Decision

**Escalation is a tool, at the read-only tier, on every surface that offers a tool set.** `escalate` takes a `message` for the person who asked and a `reason` for the operator — different readers, so different fields, matching how the repo separates user-facing from developer-facing text everywhere else. An optional `server` names the instance when there is one. It sits outside the `targets_existing_server` scope gate on purpose: a handoff about a server the caller can't see still has to reach a human rather than being refused as "no such server." Read-only rather than manager because the callers with the fewest tools have the fewest other ways forward.

**Calling it ends the turn.** `run_session` gains one hook — an escalation check polled after each round of tool calls — and stops there. Without that the model treats the call as a notification, keeps planning, and produces a closing reply that contradicts the page an operator just received. The check runs *after* the round rather than after each call, so a round that escalates alongside other tool calls still gets those results into the transcript; the operator notice is built from that transcript, and it should reflect everything the turn actually tried.

**The reason travels with the outcome.** `SessionOutcome.escalated: bool` becomes `escalation: Option<EscalationReason>` — `RoundBudget` or `AgentRequested { reason }` — with an `escalated()` accessor so the boolean Langfuse score and the `escalation-rate` metric keep their existing denominator. A second, categorical `escalation_kind` score carries which exit it was. `EscalationReason::as_str` and the event log's outcome constants share one vocabulary (`agent_requested`, `round_budget_exhausted`) so the Langfuse and Postgres planes join on it; a test in `events::mapping` asserts the two agree, since the pure agent core can't reach those constants to reuse them directly.

**One notice builder, five shells.** `Escalation::from_turn` pairs the loop's reason with the locators only a surface knows — a Discord jump link, a player name, an incident signal — so the two exits can't drift into being reported one way on the mention path and another on the watchdog path. The new `AgentRequested` notice leads with Gary's own diagnosis and, crucially, never mentions a round count: he stopped with budget to spare, and a notice claiming otherwise would misdescribe the very thing it exists to report. (This also removed a latent bug where the deferred and watchdog paths passed a transcript length where a round budget was expected.)

## Consequences

- The prompt now says stopping is a correct outcome, not a failure. That framing is load-bearing: a model that reads escalation as giving up avoids it and burns its budget instead, which is the behaviour being replaced. The two-attempts rule of thumb is deliberately loose, because the model can't see how much budget it has left.
- `escalation_kind` is the tuning instrument. A rising `agent_requested` share against a falling `round_budget_exhausted` share is the guardrail working; both rising means the tool set has a real gap.
- The in-game surface passes a check that always answers `None` — its tool set is a fixed read-only pair with no escalation tool, so the round budget stays its only exit. A player asking an unanswerable question gets the canned line, which is the right amount of ceremony for game chat.
- The deferred and watchdog turns run at manager tier, so they inherit `escalate`. An unattended investigation that finds something it can't fix now hands off with a diagnosis instead of spending sixteen rounds proving it can't.
- Bundling the shell's four callbacks into `SessionCallbacks` was forced by the argument-count lint and is the better shape anyway: `run_session`'s signature stays about the conversation rather than the plumbing. It takes two lifetimes — the borrow and the callbacks' own captures — because collapsing them would require every shell's closures to outlive the bundle, and none of them do.
- A direct `just operator tool escalate` call records the handoff and returns its prose with no operator DM: there is no loop to stop and no surface locators to build a notice from. Enough to check the tool works, without paging anyone for a harness run.

## Alternatives rejected

- **Page and keep working.** No loop change, but it lets the model notify an operator and then carry on with the same problem — and the user can end up reading a confident answer that contradicts the page. The exit has to be an exit.
- **Widen the round budget.** Treats the symptom. A model with no way to say "I'm stuck" thrashes for longer, and the failure mode that matters (inventing an answer rather than admitting a gap) isn't about budget at all.
- **Infer escalation from the reply text.** A heuristic over model output, which is exactly the kind of thing that silently stops matching when the prompt is tuned. A tool call is an unambiguous signal.
- **Replace the boolean `escalated` score with the categorical one.** The escalation-rate metric is defined on a boolean carried by *every* turn; making it categorical would drop the denominator and quietly break a documented go/no-go signal ([`docs/operations/data-collection.md`](https://github.com/Grizzly-Endeavors/grizzly-gameservers/blob/pre-hackathon/docs/operations/data-collection.md)). Two scores, one always present.
