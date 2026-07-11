# ADR-007 — Deferred-task queue for the ops agent (`run_when`)

**Status:** Accepted (2026-07-11)

## Context

Gary's first real usage surfaced two UX gaps, both rooted in the same limitation — his only way to "wait" was a blocking tool (`wait_for_server`) that held the whole agent turn polling readiness for up to five minutes:

1. **Long operations bound the conversation.** Spinning up a server (or restart-and-verify) left the user staring at a typing indicator for minutes while Gary could do nothing else.
2. **No way to defer a change.** Mid-session a friend would realize they want a config change that needs a restart, but nobody wants to be kicked. There was no "do it when I'm done."

Both collapse into one primitive: *wait until `{condition}` for `{server}`, then do `{task}`* — non-blocking, so Gary enqueues and returns immediately, and batched, so several asks over a multi-hour session are handled together rather than one turn each.

The open questions were where the wait lives, how conditions are detected, what store backs the queue, and how a fired batch is authorized.

## Decision

**Bot-side polling, no supervisor pub/sub.** The watcher for each `(server, condition)` polls the supervisor's *existing* HTTP endpoints — `/status` (readiness, via `wait_for_ready_within`) and `/occupancy` (player count, via `supervisor_occupancy`). The supervisor is untouched. This was weighed against the tempting "supervisors publish condition signals to Valkey pub/sub, the bot subscribes" design and rejected: the supervisor has **no player-event stream** — it learns occupancy only by polling RCON on a timer itself (that's how the auto-updater's idle detection already works), so a push would relay the *same* polled data at no gain in precision, while requiring a Redis client baked into every game image plus LAN egress from each game pod (a new NetworkPolicy per namespace, against the bot-scoped enforcement posture of [ADR-003](003-bot-scoped-gate-enforcement.md)) and a non-durable signal lost across a bot redeploy. Polling is also strictly more robust for multi-hour waits: watchers are rebuilt from the durable queue on startup, so a pending wait self-heals across the frequent CI/Flux redeploys. Pub/sub is explicitly deferred; it can be layered later as a latency optimization if a use case ever needs sub-poll reaction, which none does today.

**Valkey for the durable queue.** Each `(server, condition)` is a Redis list at `gameservers:wait:{server}:{condition}` on the shared foundation kv-cache (Valkey, [grizzly-platform ADR-056]); list elements are JSON task payloads. Durability is the point — the bot redeploys on every push to `main`, and a multi-hour "restart when idle" wait must survive that, which an in-memory queue (like the session store) would not. Valkey's `valkey.md` integration guide explicitly sanctions "light queues"; the eviction caveat (`allkeys-lru` under 2 GB) is accepted because the keys are few, small, and short-lived, with a 24 h TTL backstop only to reap orphans. Draining uses a per-element `LPOP` loop, whose atomicity guarantees no task is double-executed even if a transient duplicate watcher races. The bot connects the same way it reaches foundation Postgres/S3 — a LAN IP (`10.0.0.200:6379`) behind a one-line Cilium egress carve-out (`cluster/guardrails/bot-to-kv-cache-egress.yaml`) and a password synced from OpenBao (`stores/kv-cache`) via ESO. Absent password ⇒ the feature degrades (Gary reports he can't schedule), the same graceful shape as DB/S3/Ollama.

**Three conditions; `startup` is a watchdog.** `run_when` (a manager-tier tool replacing `wait_for_server`) takes `startup`, `empty`, or `idle`. `empty` fires the moment player count hits zero (for changes wanted ASAP as people log off); `idle` fires only after the server has been continuously empty for a grace window (~5 min, for no-rush tweaks that shouldn't trip on a momentary disconnect); Gary is prompted to pick by urgency and to ask when unclear. `startup` is not merely "it's up" — it wakes when a (re)start *settles* either way: healthy and accepting players, **or** failed (crashed, boot-looping, or still not up after a 20-minute ceiling — no real server takes that long, so exceeding it is itself the stuck signal). Every watcher terminus, success or failure, injects a batch so Gary reports back; nothing is silently dropped. `empty`/`idle` are refused at enqueue time for a game that reports no live player count (there'd be no way to tell when it's empty).

**Shared queue, batch runs at the manager tier.** One queue per `(server, condition)` across all users; a fired batch runs as a single Gary turn at `AccessLevel::Manager`, scoped to the target server's own guild (resolved from its label, as the in-game path does). This is justified because `run_when` is itself manager-tier and ~all deferred work is server config changes (manager-tier operations); admin-only destructive actions (destroy/restore) aren't deferrable through it and shouldn't be — they need live confirmation. The batch reuses Gary's Discord-free core (`run_gary_turn`, extracted from the mention path) and delivers the result proactively to the originating channel via the same non-gateway `Http` handle the operator notifier already uses.

## Consequences

- A friend can ask for a slow or disruptive change and keep chatting; Gary schedules it, carries it out himself when the condition is met, and comes back with the result in the channel. There is no separate notification system, and the prompt tells Gary not to promise a "ping."
- The supervisor and game images are unchanged — no new dependency ships into every game pod, and the enforcement posture stays bot-scoped. The only new infra surface is the bot → Valkey egress carve-out.
- Pending waits survive a bot redeploy: `DeferRuntime::reconcile` scans Valkey on the gateway's first `Ready` and re-arms a watcher per pending key. A watcher exits cleanly on shutdown leaving the queue intact, so the drain isn't blocked for a wait's full ceiling.
- Correctness against the drain/retire race rests on `LPOP` atomicity plus a deregister-then-recheck on exit, not on the in-memory watcher registry (which is best-effort dedup). A transient duplicate watcher is harmless.
- `startup` doubles as a boot-health watchdog: a config change that crash-loops a server now wakes Gary to investigate and escalate, even when nobody is watching Discord.
- Pub/sub remains available as a future latency layer, but polling is the durable, self-healing baseline and is expected to stay sufficient at friends-scale.

[grizzly-platform ADR-056]: ../../../grizzly-platform/docs/decisions/056-redis-to-valkey.md
