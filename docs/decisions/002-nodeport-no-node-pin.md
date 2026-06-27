# ADR-002 — NodePort routing, no node-pinning

**Status:** Accepted (2026-06-27)

## Context

The public edge (Hetzner VPS) DNATs the `7000–7010` range over `wg0` to a single cluster node (`10.0.0.226`, the control-plane). Agones' default `Dynamic` port policy uses a raw hostPort, reachable only on the node the pod lands on. Reaching a pod from the static edge therefore required either pinning every game-server pod to the edge node, or a routing layer. Node-pinning was rejected — servers should land wherever there's capacity.

## Decision

Expose game servers via **NodePort Services in the `7000–7010` range** (`portPolicy: None` on the GameServer; reachability is the Service, not hostPort). kube-proxy opens the NodePort on every node and routes to the pod wherever it is, so the static edge target needs no per-allocation change. To allow NodePorts below the default floor, the apiserver `--service-node-port-range` is widened to `7000-32767`.

Rejected alternative: Agones-native hostPort + a per-allocation edge-programming control path. It preserves client IP and uses Agones' allocator natively, but reintroduces a cluster→edge firewall-mutation path — exactly the blast-radius surface the guardrails exist to avoid. Client IP doesn't matter here (Minecraft/Valheim ban by account, not IP).

## Consequences

- Pods land on any worker; the edge stays static and untouched. Verified: a server on `intel-nuc` is reachable through the edge that targets the control-plane.
- `externalTrafficPolicy: Cluster` is required (a node without the pod must still forward), which SNATs — the pod sees a node IP, not the player's. Acceptable at friends-scale.
- The shim owns port assignment from the `7000–7010` band and creates a NodePort Service per server (friends-scale: ≤ ~11 concurrent). This is why "no more managing ports" holds for the user — the system assigns them.
- Widening the NodePort floor to `7000` is a one-time control-plane change, captured in IaC (`grizzly-platform` kubeadm-config) and the live kubeadm ConfigMap.
