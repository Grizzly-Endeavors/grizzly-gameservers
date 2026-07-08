# Sidecar Agent Interface — Exploration

**Status:** Partially implemented. The **process-supervision + lifecycle control**, the **file-mutation**, the **RCON console-bridge (`send_command`)**, and now the **in-game-trigger (reverse loop)** have shipped (`crates/supervisor/`, `crates/control-api/`, and the bot's `crates/bot/src/agones/` + `crates/bot/src/ingame/`); see "What shipped" below. The agent-facing **dock** framing remains exploratory.

## What shipped

A thin Rust supervisor (`grizzly-supervisor`) is **baked into the game image as its entrypoint** (`games/minecraft/Dockerfile`), launching the game server as a child process. It owns the Agones SDK lifecycle (calls `/ready` once the game accepts connections, then keeps `/health` pinged — including while the game is intentionally paused, so the pod survives) and serves an HTTP control API on `:9359`. The Discord bot drives it for in-place lifecycle:

- `/stop` → pause the game process; pod stays up (fast resume) → **Paused**.
- `/shutdown` → delete the `GameServer` (pod gone), keep Service+PVC → **Stopped** (cold resume).
- `/restart` → bounce the process in place.
- `/start` → state-aware: warm (pod up) resumes via the supervisor; cold (shut down) reschedules.

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

## In-game → agent triggers — **shipped**

Players inside a running server can address the agent directly, closing the reverse loop. This is live for Minecraft.

**Mechanism (as built):**

1. The supervisor's chat watcher (`crates/supervisor/src/chat_watcher.rs`) taps the captured stdout line stream continuously.
2. It pattern-matches genuine player chat (`<player> message`) in the game's chat dialect — selected per-game by `SUPERVISOR_CHAT_FORMAT` (Minecraft today) — for the trigger `@Gary` (`SUPERVISOR_CHAT_TRIGGER`, case-insensitive). Only the `<player>` shape matches, so the agent's own `tellraw` replies can't re-trigger it, and a per-player cooldown throttles spam.
3. It POSTs `{server, player, message}` (`IngameTriggerRequest`) to the bot's agent endpoint (`SUPERVISOR_AGENT_URL`) with a shared bearer token (`SUPERVISOR_AGENT_TOKEN`). The GameServer name is included so the bot maps the trigger to a channel scope without a separate dock step.
4. The bot's endpoint (`crates/bot/src/ingame/`) authenticates the token, returns `202` immediately, and runs a **read-only** tool-calling session (`list_servers`/`server_status` only, scoped to that server's channel) on Gary's shared core.
5. It routes the answer back in-game over the existing RCON `/announce` bridge (`tellraw @a`), so the whole world sees `Gary: …`.

**Why this is simple:** The bot is a long-running container, so the endpoint just runs alongside the Discord gateway in the same process, sharing Gary's core and session store. The reply reuses the already-shipped `/announce` path — no new outbound plumbing.

**Resolved decisions:**

- **Trigger:** `@Gary` (case-insensitive), matching Discord — not the `!agent` placeholder. Overridable per-game via `SUPERVISOR_CHAT_TRIGGER`.
- **Authorization:** in-game askers have no Discord identity, so they get a strict **read-only** subset (lookups only — no file reads, since a game's config can hold secrets like the RCON password, and nothing mutating). Mutating requests are deflected to an admin in Discord. See ADR-005.
- **Reply visibility:** broadcast to the whole world (`tellraw @a`). Mirroring in-game Q&A into the Discord channel is still a deferred product decision.
- **Response length:** the prompt constrains Gary to one or two short plain-text sentences (game chat is cramped), with a defensive character cap on the broadcast.
- **Endpoint authn:** a shared bearer token (advancing issue #2's token pattern in the reverse direction); the game-pod → bot path is also NetworkPolicy-scoped. An unprovisioned token degrades to NetworkPolicy-only.

## Open questions

- ~~Should the sidecar be a sidecar container or baked into the game server image?~~ **Resolved: baked into the game image as the entrypoint.** A separate sidecar container can signal the game process but can't relaunch it — once the game (the container's PID 1) exits, the container exits and the pod reschedules (the slow cluster path). Making the supervisor PID 1, with the game as its child, is what enables in-place stop/start/restart while the pod, PVC and Agones allocation all survive. The cost is a custom image per game (`FROM <upstream>` + the binary), gate-signed in CI.
- ~~Process supervisor model vs. log-file + RCON model?~~ **Resolved: process supervisor.** RCON can issue in-game commands but can't restart the server *process* — a Minecraft `/stop` just exits the process, which (without a supervisor) bounces the pod. The supervisor owns the child's lifecycle directly; graceful stop rides itzg's existing SIGTERM→world-save, so no RCON dependency for lifecycle. RCON is used, separately, for the *agent's* in-game `send_command` — now shipped via `POST /command` and Gary's `send_command` tool (see "What shipped").
- ~~In-game trigger prefix is TBD.~~ **Resolved: `@Gary`** (case-insensitive), matching Discord, overridable per-game via `SUPERVISOR_CHAT_TRIGGER`.
- ~~Response length and formatting.~~ **Resolved:** the in-game system prompt constrains Gary to one or two short plain-text sentences, with a defensive character cap on the broadcast.
- Control-port authn is NetworkPolicy-only for the bot→supervisor direction; a shared bearer token is tracked in issue #2. The supervisor→bot reverse direction (the in-game endpoint) **does** carry a shared bearer token now — the same pattern issue #2 wants applied to the control port.
- Whether to mirror in-game Q&A into the server's Discord channel is still open — a product decision, deferred.
