# ADR-010 — Crash escalation suspends the health heartbeat instead of stopping it

**Status:** Accepted (2026-08-01)

## Context

The supervisor decides when its own pod should die. It pings the Agones SDK `/health` on a cadence — even while the game is intentionally paused — so a pod is only torn down when the supervisor wants it torn down. The one thing that wants it is crash escalation: enough child crashes inside the crash window and the heartbeat stops, Agones marks the `GameServer` `Unhealthy`, and the pod is recreated.

That heartbeat was a one-way stop. `health_loop` returned on a `Notify`, and nothing could ever start it again.

The platform's other safety story assumes the opposite. Every config change Gary makes is snapshot → apply → restart → verify → auto-rollback: the file's previous content is copied aside before the write, and if the restarted server comes back `Crashed` the bot restores that snapshot and restarts again. The whole design of the per-instance mod-management overlay leans on it — that is why the supervisor seeds an *empty* `.grizzly/env`, so even the first mod install is an overwrite with something to roll back to.

Live verification of that overlay (issue #53) showed the two mechanisms racing, and escalation winning by an order of magnitude. A nonexistent Modrinth slug is a hard boot failure: itzg exits non-zero in a few seconds, five crashes accumulate well inside the crash window, and the pod was condemned in roughly 25 seconds — against a bot-side readiness ceiling measured in minutes. Worse, the race was unwinnable in principle rather than merely tight: because the heartbeat could not resume, repairing the file changed nothing. The observed run had a correctly repaired server running healthily for 90 seconds before Agones killed it anyway.

So the guardrail the design advertised did not exist. A typo cost a pod recreation and, for a NodePort-leased instance, potentially a new address for the friend — and if the bot's verify had timed out inconclusive, the recreated pod would read the same bad file off the same PVC and crash-loop identically.

## Decision

**The heartbeat is a switch, not a stop.** Escalation suspends it; a relaunched child that reaches readiness resumes it and gets a fresh crash budget.

Readiness is the resume signal because it is the one thing a broken server cannot fake. A child that still crashes never reaches it, so a genuinely broken pod is condemned on exactly the old schedule. Only a repair gets that far — which makes the bot's existing rollback (restore the snapshot, restart) sufficient to save the pod, with no new machinery on either side of the wire.

The crash budget is refunded **only** on that recovery, never on ordinary readiness. A game that dies partway into every boot reaches readiness each cycle; refunding there would reset the counter forever and turn a crash loop into a permanent one.

Minecraft's catalog health budget widens correspondingly — `failureThreshold` 5 → 10, so 150s of missed pings rather than 75s. That window is now a recovery budget: the repaired boot has to reach readiness inside it, and a modded Minecraft boot re-resolves its Modrinth downloads before the world even loads.

Measured on the bench after the change: escalation at 55s, rollback issued immediately, running again 39s later, with Agones never leaving `Ready` and the pod never restarting.

### Rejected: raise the crash threshold for Minecraft

Widening `SUPERVISOR_CRASH_THRESHOLD` so the bot's verify window can win the race treats the symptom. To beat a minutes-scale ceiling with a child that fails in seconds, the threshold would need to be tens of crashes — which is no longer a crash-loop guard in any meaningful sense, and it leaves the "a repair cannot rescue the pod" half of the defect untouched.

### Rejected: exponential relaunch backoff

Backing off harder between crash relaunches delays escalation, buying the verify loop time. But the bot only *learns* there is a problem when escalation flips the phase to `Crashed`, so delaying escalation delays the rollback by the same amount. It moves the deadline without changing who wins, and it slows recovery from ordinary transient crashes as a side effect.

### Rejected: validate the value before writing it

Checking a Modrinth slug against the registry before it reaches the overlay is worth doing on its own merits, and it lands with the discovery tools. It is not a substitute: it prevents one known bad value on one game, while escalation races every hard-failing config change on every game. The guardrail has to hold for the case nobody predicted.

## Consequences

- Escalation stopped being a one-way door. Reading `crash threshold exceeded` in a log no longer means the pod is doomed — it means the pod is on the clock, and the following lines say whether it recovered.
- A pod that nothing repairs behaves exactly as before, just with a wider Minecraft window (150s rather than 75s) before Agones recreates it.
- The recreation-loop case is unchanged and still open: a bad overlay value that no one rolls back is re-read by the recreated pod off the same PVC. Pre-write validation is the answer there, not this.
- `Heartbeat` is a general supervisor primitive, not a mod-management one. Any future reason to go quiet and come back inherits the same semantics.
