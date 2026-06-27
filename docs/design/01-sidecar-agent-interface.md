# Sidecar Agent Interface — Exploration

**Status:** Exploration / pre-implementation. No decisions finalized.

## The question

How should the ops agent interact with running game server containers? Two broad approaches were on the table: wrapping `kubectl exec` calls as agent tools, or shipping a thin Rust client into each game server pod as a sidecar.

## Why not exec-as-tools

The exec approach gives the agent effective shell access to running game servers — arbitrary commands over a cold exec session each call. That directly undercuts the snapshot→apply→verify→rollback guardrail the design depends on, because there's no enforcement layer between the agent and the container. Exec sessions are also inherently unstable: each call starts a new session that can fail mid-sequence, and if the container restarts between the snapshot and the write the operation is silently lost.

## Sidecar model

A thin Rust binary runs as a sidecar container in each Agones `GameServer` pod. The agent gets a typed tool surface; the sidecar enforces invariants.

### Dock metaphor

The agent's tool surface has two layers:

- **Fleet-level tools** — `list_servers()` returns running Agones GameServer instances.
- **Session-level tools** — `dock(server_id)` establishes a persistent connection to that server's sidecar. Within a docked session the agent gets a constrained set of tools: `ls`, `read_file`, `write_file`, `tail_log`, `send_command`, `rollback`.

`cd` is intentionally omitted — all paths are absolute to avoid working-directory drift across turns.

### File mutation guardrail

`write_file` is atomic at the sidecar level: it snapshots the existing file before writing and returns a `snapshot_id`. The agent has a `rollback(snapshot_id)` tool. The verify step (did the server come back up) is separate from the write — the rollback path is always available without the agent having to orchestrate a pre-write backup step itself.

## Console access

Since the sidecar is already in the pod, it can bridge the game server's console in both directions.

### Output (read)

Most game servers write logs to a file (`logs/latest.log` for Minecraft). `tail_log(path, n_lines)` is already covered by the file-read surface. No special plumbing required.

### Input (commands)

Two channels depending on what the game supports:

- **RCON** — Minecraft, Source engine games, and several others support it natively. The sidecar runs an RCON client and exposes `send_command(cmd)`. Needs RCON enabled in the game config, which the per-game template in `games/` can enforce. This is the preferred channel for supported games.
- **stdin pipe** — For games without RCON, if the sidecar is the process supervisor (launches the game as a child process and owns its stdin), it can write commands directly. Adds process lifecycle responsibility to the sidecar; deferred until needed.

The `send_command` tool signature is the same regardless — per-game sidecar config declares which channel to use.

## In-game → agent triggers

Players inside a running server can address the agent directly, closing the reverse loop.

**Mechanism:**

1. Sidecar watches the log stream continuously.
2. Pattern-matches player chat for a trigger prefix (e.g., `!agent <message>`).
3. Parses the player name and message body; posts them as an HTTP request to the agent service endpoint.
4. Agent handles it like any other request — server ID is included in the request body so the agent can look up the right Agones GameServer without needing a separate dock step.
5. Sidecar routes the response back in-game via RCON (`say` or `tellraw`). For Minecraft, `tellraw` is preferred — it allows colored/formatted JSON chat so agent replies are visually distinct from player chat.

**Why this is simple:** The bot is a long-running container, not an on-demand service. The agent is baked into the bot process. The sidecar just needs the bot's internal service endpoint — no new infrastructure, no ephemeral invocation.

Friends watching the Discord server won't see in-game queries unless we explicitly mirror them. That's a product decision for later.

## Open questions

- Should the sidecar be a sidecar container (separate container, shared pod network) or baked into the game server image? Sidecar container is preferred — avoids touching game server base images and keeps the binary separate from the game process.
- Process supervisor model (sidecar owns game server stdin/stdout as a child process) vs. log-file + RCON model: the latter is simpler and sufficient for the games likely in scope. Revisit if a game needs stdin and has no RCON.
- In-game trigger prefix is TBD — `!agent` is a reasonable default but may conflict with game commands or other bots.
- Response length and formatting: game chat has line-length limits. The agent prompt will need instructions to keep in-game responses short and plain-text.
