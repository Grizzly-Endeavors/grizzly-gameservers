# games/_template

Skeleton for onboarding a new game. The manifests here carry a deliberate security baseline so a new game is **gate-clean and PodSecurity "restricted"-compatible by default** — don't strip it without a reason. Every game's image bakes in the grizzly supervisor as PID 1 (wrapping the game as its child), which owns the Agones SDK lifecycle and serves the in-pod control API the bot/Gary drive. `minecraft/` is the worked example (its `gameserver.yaml` documents one deliberate exception to the baseline — root at startup — that predates and doesn't apply to a fresh game).

## Onboarding a game

1. Copy this directory: `games/_template/` → `games/<game>/`.
2. Fill in `Dockerfile`: `REPLACE_BASE_IMAGE`/`REPLACE_DIGEST` (the game's own upstream image, pinned), `REPLACE_CHILD_CMD` (the command the supervisor launches), and the optional `SUPERVISOR_DATA_DIR`/`SUPERVISOR_RCON_PORT` env vars if they apply. See `games/minecraft/Dockerfile` for a filled-in example.
3. Replace every `REPLACE_*` placeholder in the manifests: `REPLACE_GAME` (name/selector/labels), `REPLACE_IMAGE` (the image this directory's `Dockerfile` builds — a tag or digest, never floating `:latest`), `REPLACE_PORT` / the `containerPort` / `nodePort` (a free port in **7000–7010**, the edge-forwarded band), the data `mountPath`, env, and resource sizing.
4. Add `<game>` to `games/kustomization.yaml` so Flux renders it.
5. Validate against the live Agones webhook before pushing: `kubectl apply --dry-run=server -k games/<game>/`.

## The security baseline (why each piece is here)

- **Pod `runAsNonRoot: true` + `runAsUser`/`runAsGroup` + `fsGroup`** — no root in the pod; `fsGroup` makes the data PVC writable by the runtime user. Set `runAsUser` to the UID the game image actually runs as (itzg/minecraft-server = 1000).
- **`seccompProfile: RuntimeDefault`** + container **`capabilities: drop [ALL]`** + **`allowPrivilegeEscalation: false`** — restricted-PSS posture; no privilege escalation, no extra kernel surface.
- **`readOnlyRootFilesystem: true`** — the game may only write to mounted volumes. The skeleton mounts a writable `/tmp` (emptyDir) and a data PVC; add an emptyDir for any other path the image writes to. If a game genuinely can't run read-only (or its startup needs root), document why before relaxing it — see `games/minecraft/gameserver.yaml`'s exception for the pattern to follow.
- **Whole-CPU limits** — fractional CPU limits throttle and the gate flags them; size the limit to the game or omit it to allow bursting.
- **The supervisor, not a readiness sidecar** — earlier revisions of this template bridged a game to the Agones lifecycle with a separate `agones-ready` busybox sidecar. That's been replaced everywhere: the supervisor is baked into the game image itself (this directory's `Dockerfile`), because a separate sidecar can *signal* the game process but can't *relaunch* it — see `docs/design/01-sidecar-agent-interface.md`.

## Notes

- **Data directory for the ops agent (`SUPERVISOR_DATA_DIR`)**: the supervisor scopes Gary's file tools (`browse_files`/`read_file`/`write_file`/`restore_file`) to this directory, defaulting to `/data`. If your game's data PVC `mountPath` is anything other than `/data`, set `SUPERVISOR_DATA_DIR` to match in the `Dockerfile`'s `ENV` — so the agent can reach the game's config and logs. Paths outside this directory (absolute paths, `..`) are refused.
- **RCON / console access**: only wire `SUPERVISOR_RCON_PORT` (and the matching supervisor dialect flag) if the game speaks it and you want Gary's `send_command` tool to issue console commands. See `games/minecraft/Dockerfile` for the worked example, including ephemeral password minting so no secret is baked into the image or any Kubernetes object.
- **UDP games** (Valheim, etc.): set `protocol: UDP` on the port and Service. The edge forwards UDP+TCP, but TCP MSS clamping doesn't help UDP — a game sending >~1380-byte datagrams needs its own MTU/payload setting (the `wg0` tunnel is 1420). See `docs/activation-status.md`.
- Promote a single `GameServer` to a `Fleet` + shim-managed per-instance NodePort Services once the ops agent/shim owns allocation.
