# ADR-037 — A running turn is stopped cooperatively, and a stop is not an escalation

**Status:** Accepted (2026-09-10)

## Context

A turn is the unit that costs money: one message to Gary buys up to `gary.max_rounds` paid model calls, and every bound already in the system caps what a turn may *do* rather than how long it may keep doing it. The round budget ([ADR-019](019-agent-invoked-escalation.md)) is the backstop, and until now it was the only way a turn that had stopped converging ever ended.

That was tolerable at the compiled default of 16 rounds. It stopped being tolerable once the budget could be raised at runtime ([ADR-024](024-runtime-settings.md)) — a setting that exists precisely so a hard problem can be given more room, which is the same knob that lets a thrashing turn run far longer than anyone watching wants to wait. The observed case was three consecutive `round_budget_exhausted` turns on a question the platform could not answer at all, followed by the budget being raised to 200 to see how far the model would get. At that size, nobody watching a runaway has any recourse but to wait for it.

Two things made this more than a convenience. Gary's tools restart game servers, write config files, and issue in-place resizes against worlds real friends are playing in — so "stop" cannot mean "drop whatever you were doing". And the escalation rate is the product's headline metric, defined on a boolean carried by every turn, so however a stop is recorded it must not quietly become part of that number.

## Decision

**A per-channel cancellation token the agent loop observes between rounds and races against the model call, surfaced as `/gary-stop`.**

### The model call is abandoned; a tool never is

The loop races each model call against the token and re-checks it after each round of tools. A stop therefore lands on the model — which is where a runaway turn spends nearly all of its wall-clock — while a tool already in flight always runs to completion.

The asymmetry is the whole point. Abandoning a model call costs only the tokens already spent, and the request is idempotent to drop. Abandoning a tool is not: an `edit_file` whose verifying restart never happens leaves a config change with no watcher and no rollback, and a `restart_server` torn up mid-flight leaves a world bounced with nothing confirming it came back. The worst case is that a stop waits out one tool round; the alternative's worst case is a half-applied change on a live server, which is exactly what the snapshot-verify-rollback machinery exists to prevent.

The check appears in two places rather than one. The top-of-round race is what makes a stop feel instant. The bottom-of-round check exists only so a stop that arrives during the *final* round is reported as a stop, rather than falling out of the loop and being recorded as the round budget running out on its own.

### A stop is its own ending, not an escalation

`SessionOutcome` carries a `TurnEnding` — answered, escalated, or cancelled — rather than an `Option<EscalationReason>` plus a flag. Three reasons:

- **The metric stays honest.** `escalated` is a boolean on every turn so the escalation-rate denominator holds ([ADR-013](013-langfuse-span-construction.md)). A stop is an operator action, not the agent failing to finish; folding it in would mix the two and make the headline number unreadable exactly when somebody started using the stop command. Cancellation gets its own boolean score, `cancelled`, on the same every-turn basis and for the same reason.
- **Nobody is paged.** An escalation DMs the operators and writes an `escalation` row. A stop must do neither — the person who stopped it already knows, and waking an operator because somebody clicked stop is the fastest way to make the notification worthless.
- **The compiler asks the question.** `wildcard_enum_match_arm` is denied, so every site that consumes an outcome had to answer for the new ending rather than inheriting whichever arm happened to be the catch-all. A fourth ending will get the same treatment.

The stop is still appended to the transcript as an assistant turn. Without it the next message in the channel sees the question with nothing after it and answers it again — the worst possible response to a request somebody deliberately stopped.

### Channel-scoped, and ungated

The conversation belongs to the channel ([ADR-020](020-channel-bound-conversations.md)) and the session store already serialises turns per channel, so there is exactly one turn to stop and no ambiguity about which. The command is ungated for the same reason `/new-session` is: the transcript is shared, so anyone who can add to it can end a turn against it, and the cost of a stop nobody wanted is that somebody asks again. Gating it behind manager would mean the read-only friend who started a runaway is the one person who cannot end it.

The confirmation is posted to the channel rather than ephemerally, because a shared conversation going quiet mid-answer reads as a fault to everyone who didn't run the command.

### Rejected: aborting the task

Dropping the turn's `JoinHandle` is instant and needs no plumbing in the loop. It also abandons tools mid-mutation, skips the transcript commit so the exchange vanishes, and leaves the Langfuse trace unclosed. Instant is not worth any of those on a system that operates live worlds.

### Rejected: a round-boundary check only

One more synchronous check beside `escalation_requested`, with no `select!` and no cancellation reaching the model call. Much smaller, but a stop then lands only after the in-flight generation returns — which on a slow generation is the exact wait the command exists to cut short. A stop button that does nothing for thirty seconds is one people press repeatedly and then stop trusting.

### Rejected: an admin-only "stop everything"

Genuinely useful as a panic button, and reachable later from the same registry. Left out for now because the failure it addresses — several concurrent runaway turns at once — has not happened, and the per-channel command covers every case that has.

## Consequences

- The in-game surface is not stoppable. A player's `@Gary` carries no channel to scope a stop to, so it passes a token nobody holds and behaves exactly as before. The throttle's per-(server, player) bucket remains that surface's only bound.
- Watchdog and deferred turns are likewise unstoppable today. Both post into a channel, so extending the registry to cover them is a small change if it turns out to be wanted; neither has an asker waiting on it, which is what makes it less urgent.
- A stopped turn still reports the generations it paid for. A cost report that omitted them would understate exactly the turns most worth looking at.
- `events` gains a `turn_cancelled` kind, attributed to whoever ran the command rather than whoever started the turn — in a shared channel those are often different people, and the useful audit answer is who pulled the plug.
- Worst-case stop latency is one tool round. On a `restart_server` against a cold-starting game that can be tens of seconds, and the command's own wording says so rather than promising an instant stop it can't deliver.
