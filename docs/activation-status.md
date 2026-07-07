# Activation Status — what's live

The infrastructure is up. The cluster runs Agones, the guardrails, and a live Minecraft server reachable from the internet. What remains is the application (Discord shim + ops agent) — which is now developed against this real infra, not mocks.

## Live (verified)

- **Edge forwarding** — Hetzner VPS + R730xd forward the **7000–7010** UDP+TCP range over `wg0` to the cluster (`grizzly-platform` ansible; ADR-019). Validated end-to-end: `<vps-public-ip>:7000` reaches the Minecraft pod and a real client joins. **Requires TCP MSS clamping** on the `wg0` forward path (`ingress-tunnel` role `wg0.conf` `PostUp`) — game traffic is L3-forwarded, so without clamping to the 1420 tunnel MTU, login succeeds but world-load packets black-hole. UDP games (later) can't use MSS clamping and will need their own MTU handling.
- **Agones `1.58.0`** — installed as its own Flux `HelmRelease` (`cluster/agones/`), operator + CRDs Ready in `agones-system`. Watches `game-servers`; its own hostPort range pinned to `8000–8100`, clear of the NodePort band. See ADR-001.
- **Guardrails** (`cluster/guardrails/`) — `game-servers` namespace, the `ops-agent` ServiceAccount + scoped Role/RoleBinding, and the `CiliumNetworkPolicy` egress leash. Leash verified: from a game pod, LAN/platform services (OpenBao `10.0.0.200:8200`) are blocked while DNS, the Kubernetes API, and the internet work.
- **Game catalog** (`games/minecraft/`) — a live Minecraft `GameServer` (composite image: grizzly-supervisor as PID 1, wrapping itzg/minecraft-server and driving the Agones SDK), a NodePort `Service` on `7000`, and a world PVC. Reaches Ready; **lands on a worker, not the edge node** — kube-proxy routes the static edge to the pod wherever it runs (no node-pinning). See ADR-002.
- **Flux wiring** (`grizzly-platform`) — `grizzly-gameservers-cluster` (path `./cluster`, `wait: true`) and `grizzly-gameservers-games` (path `./games`, `dependsOn` cluster), both reconciled from the `grizzly-gameservers` GitRepository on `main`. The app `HelmRelease` renders `./deploy`, which now carries the OpenBao-backed `ExternalSecret`s (`discord-bot`, `ollama-api`) — the bot/agent workload itself is still pending.
- **NodePort range** — apiserver `--service-node-port-range` widened to `7000-32767` (IaC in `grizzly-platform` kubeadm-config; live + kubeadm ConfigMap reconciled) so game Services bind `7000–7010` 1:1 with the edge.

## Not yet live (next — application layer)

- **`deploy/` chart** — real bot/agent workload templates; the empty chart renders nothing today.
- **Discord shim + ops agent** — the Rust app. Builds against the live Agones allocation API + `ops-agent` RBAC + a real server now.
- **CI gate** (`.github/workflows/deploy.yml`, currently `if: false`) — flip on once there's an image to build and sign.

## Deferred

- **Full-namespace gate enforcement** — `game-servers` already carries `grizzly.io/gated=true`, but enforcement is bot-scoped only (ADR-003); the Agones SDK sidecar needs no carve-out (it's a `gcr.io` image the policy glob never matches), but game/supervisor images aren't yet gate-signed in CI, so full-namespace enforcement stays a documented follow-up (see `cluster/kyverno/README.md`).
- **Fleet + per-instance NodePort Services** — the single standalone Minecraft `GameServer` becomes a `Fleet` with shim-managed per-server Services once the shim exists.
