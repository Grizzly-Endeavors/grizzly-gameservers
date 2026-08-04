# ADR-017: Bearer-token auth on the supervisor control API

## Status

Accepted.

## Context

The in-pod supervisor serves an HTTP control API on `9359`. The bot drives lifecycle actions through it (`/stop`, `/start`, `/restart`, `/status`), reads and writes files under the instance's data directory, tails logs, runs console commands, and streams whole worlds in and out for backup and restore.

For the first cut, access was gated by **network identity alone**. The `game-servers` egress leash default-denies pod egress and `cluster/guardrails/bot-to-supervisor-egress.yaml` opens exactly one path: bot identity → game pods, TCP 9359. Game pods cannot reach each other's control ports, and 9359 is never exposed through the NodePort edge.

That control is real, but it is single and it fails open. Anything that acquires the bot's network identity — a CNI policy gap, a mislabelled pod, a compromised workload scheduled into the namespace — inherits full lifecycle and filesystem access to every game pod, with nothing at the application layer to stop it. The blast radius is every friend's world.

## Decision

A shared bearer token, `SUPERVISOR_CONTROL_TOKEN`, synced from OpenBao into the `grizzly-gameservers-control` Secret and read by both halves: the bot presents it on every control call, the supervisor requires it on every control route.

Three things about the shape are load-bearing.

**Every route, not just the mutating ones.** The original follow-up scoped this to `/stop`, `/start`, `/restart`, which is what existed when it was written. Since then `GET /fs/read` returns any file under the instance's data directory and `GET /archive` streams the entire world; those reads are worth as much to an attacker as the lifecycle calls. It is applied as one middleware over the whole router rather than per route, so a route added later is protected by default instead of by remembering to classify it.

**Optional on both sides, and both sides read blank as absent.** An unset or empty token leaves the API on the network-leash-only posture. This is deliberate: the bot and the game pods are deployed independently — the bot through gated CI, game images by hand — so a mandatory token would mean a window where one half enforces and the other cannot answer, and the symptom would be every lifecycle action on every server failing at once. Degrading to the previous posture makes a half-rolled deploy boring. Both sides trim and treat empty as absent, so a secret that syncs blank cannot leave them disagreeing about whether auth is on. Each side logs which posture it is in at startup, because the difference is otherwise invisible from outside.

**The credential rides its own type.** `ControlClient` wraps the bot's `reqwest::Client` and attaches the token; the supervisor-facing functions take `&ControlClient` rather than `&reqwest::Client`. The bot's general client also reaches Ollama, Modrinth, and Langfuse, so attaching the token to it as a default header would send an in-cluster credential to three third parties. Making it a distinct type means the compiler, not a reviewer, is what keeps supervisor traffic and outbound traffic apart — it caught two Modrinth call sites during the change that a hand-audit would plausibly have missed.

## Consequences

- Reaching the control port now requires both the bot's network identity and the shared secret. An in-namespace foothold no longer inherits control of every game pod.
- The token must be provisioned in OpenBao at `grizzly-platform/gameservers/control → token` (`openssl rand -hex 32`) before it does anything. Until then both halves run as before and say so at startup.
- Rotation requires the bot and the game pods to be carrying the same value. Because absent degrades rather than fails, the safe order is: write the new token, roll the game images, roll the bot. A mismatch — as opposed to an absence — fails closed with 401s, which is the correct behaviour but is a real outage of lifecycle actions until it is reconciled.
- One secret is shared by every game pod in the namespace, so a single compromised game pod exposes the token for all of them. That is accepted at friends scale: the alternative, a per-instance credential minted at provision time, is meaningfully more machinery for a threat model in which the game pods are all equally trusted. Revisit it if game pods ever run untrusted third-party code.
- It does not replace the network policy. Both stay; the policy is the outer control and this is the inner one.
