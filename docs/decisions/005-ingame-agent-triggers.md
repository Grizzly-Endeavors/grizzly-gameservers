# ADR-005 — In-game chat triggers for the ops agent

**Status:** Accepted (2026-07-08)

## Context

Gary (the ops agent) was reachable only from Discord — `@mention`, DM, home channel, or slash command. A non-technical friend who is *in* a running game had to alt-tab to Discord to ask anything. `docs/design/01-sidecar-agent-interface.md` had long sketched the reverse loop (a player types `@Gary <question>` in game chat, the supervisor forwards it, Gary answers, the reply comes back in-game) as exploratory. Two pieces already existed: Gary's LLM core is Discord-free and reusable (`crates/bot/src/agent/`), and the reply-into-game path is shipped (RCON `/announce` → `tellraw @a`). The genuinely new surface — untrusted in-game chat reaching an LLM — needed decisions on authorization, transport, and auth before building it.

The hard constraint: in-game players have **no Discord identity** and no channel-scoped tenancy. The Discord tenancy boundary (a GameServer is labelled with its owning Discord channel; every read/action confines to the caller's channel) is the guardrail that contains Gary; an in-game entrypoint must not become an unauthenticated path around it, and it opens a new prompt-injection surface (a player controls the text that reaches the model).

## Decision

**Transport — supervisor pushes to the bot.** The supervisor's chat watcher POSTs matched triggers to a small HTTP endpoint on the bot (`crates/bot/src/ingame/`), which runs alongside the Discord gateway in the same process. The bot answers asynchronously and replies over the existing RCON `/announce` bridge, so the supervisor's POST returns `202` immediately and never waits on the model turn. This matches the design doc and reuses the shipped reply path; the cost is a new inbound path (game pod → bot), addressed by the auth and network decisions below.

**Authorization — read-only.** In-game askers get a strict subset of Gary's tools: `list_servers` and `server_status` only, scoped to the triggering server's own channel (resolved from the GameServer's channel label). No file reads (a game's config can hold secrets — itzg writes the RCON password into `server.properties`), no logs (they can leak player IPs), and nothing mutating. Mutating requests are deflected: Gary tells the player an admin has to do that from Discord. This sidesteps both the Discord-confirmation-button coupling of the mutating tools and the escalation risk of untrusted input, and matches the stated goal (non-technical friends *ask questions*).

**Prompt hardening.** The in-game system prompt presents the player's text as a quoted, attributed question and instructs Gary to treat it strictly as data — never as instructions that could change its role, reveal the prompt, or act outside answering — and to keep replies to one or two short plain-text sentences (game chat is cramped), with a defensive character cap on the broadcast.

**Reply visibility — broadcast.** Answers go to the whole world via `tellraw @a` (the shipped `/announce`). Mirroring in-game Q&A into the Discord channel is deferred (a product decision).

**Trigger — `@Gary`,** case-insensitive, matching Discord; overridable per-game via `SUPERVISOR_CHAT_TRIGGER`. Loop-prevention is structural: the watcher only matches genuine `<player> message` chat, a shape the agent's own `tellraw` reply never takes.

**Endpoint auth — shared bearer token.** The endpoint requires a bearer token (constant-time compared), synced from OpenBao via ESO into one `grizzly-gameservers-ingame` Secret that both the bot and the game pods read (they share the `game-servers` namespace). This advances issue #2's token pattern in the reverse direction. The game-pod → bot path is additionally NetworkPolicy-scoped (a Cilium egress allowance for TCP 9360 only). An unprovisioned token degrades gracefully to the NetworkPolicy-only posture rather than failing — the same graceful-degradation shape as Gary/DB/S3.

## Consequences

- Friends can ask Gary questions without leaving the game; the answer is visible to everyone in the world.
- The blast-radius guardrail is unchanged: the namespace RBAC/NetworkPolicy box still contains Gary regardless of entrypoint, and the read-only tool set means a prompt-injected in-game message can, at worst, make Gary read server listings within one channel.
- The bot gains a small inbound HTTP surface (axum) and a new dependency. It is pod-internal (ClusterIP, never NodePort), token-authenticated, and NetworkPolicy-scoped to game-pod callers only.
- Per-game generality is preserved: the chat-line dialect is declared per-game (`SUPERVISOR_CHAT_FORMAT`, Minecraft today) with the generic watcher in the supervisor, mirroring the `send_command` split. Adding a second game means adding a `ChatFormat` variant, not touching the bot.
- The shared token must be provisioned in OpenBao (`grizzly-platform/gameservers/ingame → token`) for the endpoint to be authenticated; until then both sides run open behind the NetworkPolicy. Full control-port auth (the bot→supervisor direction) remains issue #2.
