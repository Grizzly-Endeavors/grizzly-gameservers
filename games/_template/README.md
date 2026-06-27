# games/_template

Skeleton for onboarding a new game. The manifests here carry a deliberate security baseline so a new game is **gate-clean and PodSecurity "restricted"-compatible by default** — don't strip it without a reason. `minecraft/` is the worked example; this skeleton is the hardened starting point future games should copy (the live `minecraft/` GameServer predates this baseline and is a temporary bring-up artifact).

## Onboarding a game

1. Copy this directory: `games/_template/` → `games/<game>/`.
2. Replace every `REPLACE_*` placeholder: `REPLACE_GAME` (name/selector/labels), `REPLACE_IMAGE` (pin a tag or digest — never floating `:latest`), `REPLACE_PORT` / the `containerPort` / `nodePort` (a free port in **7000–7010**, the edge-forwarded band), the data `mountPath`, env, and resource sizing.
3. Add `<game>` to `games/kustomization.yaml` so Flux renders it.
4. Validate against the live Agones webhook before pushing: `kubectl apply --dry-run=server -k games/<game>/`.

## The security baseline (why each piece is here)

- **Pod `runAsNonRoot: true` + `runAsUser`/`runAsGroup` + `fsGroup`** — no root in the pod; `fsGroup` makes the data PVC writable by the runtime user. Set `runAsUser` to the UID the game image actually runs as (itzg/minecraft-server = 1000).
- **`seccompProfile: RuntimeDefault`** + container **`capabilities: drop [ALL]`** + **`allowPrivilegeEscalation: false`** — restricted-PSS posture; no privilege escalation, no extra kernel surface.
- **`readOnlyRootFilesystem: true`** — the game may only write to mounted volumes. The skeleton mounts a writable `/tmp` (emptyDir) and a data PVC; add an emptyDir for any other path the image writes to. If a game genuinely can't run read-only, document why before relaxing it.
- **Whole-CPU limits** — fractional CPU limits throttle and the gate flags them; size the limit to the game or omit it to allow bursting.
- **`agones-ready` sidecar** — bridges non-instrumented games to the Agones lifecycle (waits for the port, drives the SDK `/ready` + `/health`). Keep it unless the game speaks the Agones SDK natively.

## Notes

- **UDP games** (Valheim, etc.): set `protocol: UDP` on the port and Service. The edge forwards UDP+TCP, but TCP MSS clamping doesn't help UDP — a game sending >~1380-byte datagrams needs its own MTU/payload setting (the `wg0` tunnel is 1420). See `docs/activation-status.md`.
- Promote a single `GameServer` to a `Fleet` + shim-managed per-instance NodePort Services once the ops agent/shim owns allocation.
