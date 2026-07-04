# Sidecar Agent Interface — Exploration

**Status:** Partially implemented. The **process-supervision + lifecycle control**, the **file-mutation**, and the **RCON console-bridge (`send_command`)** portions have shipped (`crates/supervisor/`, `crates/control-api/`, and the bot's `crates/bot/src/agones/`); see "What shipped" below. The agent-facing **dock** framing and the **in-game-trigger** (reverse loop) surface remain exploratory.

## What shipped

A thin Rust supervisor (`grizzly-supervisor`) is **baked into the game image as its entrypoint** (`games/minecraft/Dockerfile`), launching the game server as a child process. It owns the Agones SDK lifecycle (calls `/ready` once the game accepts connections, then keeps `/health` pinged — including while the game is intentionally paused, so the pod survives) and serves an HTTP control API on `:9359`. The Discord bot drives it for in-place lifecycle:

- `/stop` → pause the game process; pod stays up (fast resume) → **Paused**.
- `/kill` → delete the `GameServer` (pod gone), keep Service+PVC → **Stopped** (cold resume).
- `/restart` → bounce the process in place.
- `/start` → state-aware: warm (pod up) resumes via the supervisor; cold (killed) reschedules.

It also serves the file surface (`/fs/list`, `/fs/read`, `/fs/write` with snapshot, `/fs/restore`, `/logs`) and, for games that enable it, an RCON console bridge:

- `/command` → run one in-game console command over RCON and return the reply. The supervisor **mints an ephemeral RCON password at pod startup** and injects it into the game child's environment (itzg's `RCON_PASSWORD`), then authenticates its own RCON client with the same value — so the password never touches git or a Kubernetes object and rotates each pod start. The per-game template turns RCON on (`SUPERVISOR_RCON_PORT`, plus `SUPERVISOR_RCON_MINECRAFT` to select the Minecraft dialect); the port stays pod-internal. The bot exposes this to admins as Gary's `send_command` tool.

This resolved two of the open questions below in the *opposite* direction from the original lean — see there.

---

**Original exploration follows.**

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

- ~~Should the sidecar be a sidecar container or baked into the game server image?~~ **Resolved: baked into the game image as the entrypoint.** A separate sidecar container can signal the game process but can't relaunch it — once the game (the container's PID 1) exits, the container exits and the pod reschedules (the slow cluster path). Making the supervisor PID 1, with the game as its child, is what enables in-place stop/start/restart while the pod, PVC and Agones allocation all survive. The cost is a custom image per game (`FROM <upstream>` + the binary), gate-signed in CI.
- ~~Process supervisor model vs. log-file + RCON model?~~ **Resolved: process supervisor.** RCON can issue in-game commands but can't restart the server *process* — a Minecraft `/stop` just exits the process, which (without a supervisor) bounces the pod. The supervisor owns the child's lifecycle directly; graceful stop rides itzg's existing SIGTERM→world-save, so no RCON dependency for lifecycle. RCON is used, separately, for the *agent's* in-game `send_command` — now shipped via `POST /command` and Gary's `send_command` tool (see "What shipped").
- In-game trigger prefix is TBD — `!agent` is a reasonable default but may conflict with game commands or other bots.
- Response length and formatting: game chat has line-length limits. The agent prompt will need instructions to keep in-game responses short and plain-text.
- Control-port authn is NetworkPolicy-only for now; a shared bearer token is tracked in issue #2.
