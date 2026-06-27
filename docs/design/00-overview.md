# grizzly-gameservers — High-Level Design

**Status:** Scaffold / design. No implementation yet.

## What this is

A user-friendly service that lets non-technical friends spin up and manage game servers on Grizzly Endeavors hardware through a **Discord bot**. Game servers run as containers in the homelab Kubernetes cluster; the public edge is the Hetzner proxy VPS. The friend types a command (or, later, plain English) in Discord and gets back an address to connect to. When something breaks or needs tuning, they ping the bot instead of pinging Bear.

This is explicitly **not** a "let my technical friends touch my infra" tool. The audience is non-technical, so the bot owns the whole experience and the platform stays invisible.

## Why these pieces

The shape fell out of a few hard constraints:

- **Servers run on home hardware, not the VPS.** The VPS is a proxy/edge. So "open a port" is never enough on its own — traffic has to be *forwarded* from the public edge down to the cluster. That forwarding (DNAT over the existing WireGuard tunnel) has to happen on the VPS regardless of any cloud firewall, which is why a Hetzner Cloud Firewall was rejected: it can only filter, not route, so it would become a redundant second place to keep port state in sync. The edge is managed in one place — `grizzly-platform` ansible.
- **Servers are Kubernetes containers.** That makes per-server port churn a cluster concern, not an edge concern. The edge forwards one bounded port range, set once; individual ports are handed out *inside* the cluster. The "don't leave a range open" instinct doesn't apply the way it first seems — an open port in the range with no allocated pod behind it has nothing listening, so it isn't meaningful attack surface. The real controls live in the control plane and in pod isolation, not in dynamic edge choreography.
- **Each game stores config completely differently** (gather rates, mods, mod configs, difficulty live in different files/folders/formats per game), and applying changes means restarting the server and sometimes fixing what broke. Hand-rolling a per-game config adapter for every game is exactly the maintenance pile we want to avoid. This is the core reason an LLM **ops agent** is central rather than optional: it absorbs each game's idiosyncrasy generically instead of us pre-encoding every game's layout.

## Architecture

```
Discord (friends)
   │  slash commands / NL request
   ▼
Discord shim ──────────► Ops agent (LLM loop)
   │  allocate/teardown        │  read config+logs, mutate, restart, verify, roll back
   ▼                           ▼
Agones (lifecycle, ports, allocation API, health, exec substrate)
   ▼
GameServer pods (per-game image + per-instance config on a PVC)
   ▲
   │  static port range forwarded once
Hetzner VPS edge ──wg0 tunnel──► R730xd ──DNAT──► K8s node
```

### Components

**Discord shim** — the only piece that is purely "our product." Thin: authenticate the friend, map a command to an Agones allocation/teardown, hand back `IP:port`. No quotas, no billing — this is a friends-scale service.

**Ops agent** — the center of gravity. An LLM loop that translates intent and operates running servers: read current config + logs (via Agones/k8s exec), decide the change, mutate the right file, restart the GameServer, watch health, report back. This is what replaces N hand-rolled per-game config managers. Two distinct uses, only one of which is autonomous:
- *Apply a known change* (gather rate, difficulty, add a mod) — bounded and reliable. Build this first.
- *Diagnose an arbitrary failure and fix it* — open-ended; the agent will sometimes flail here. Scope it with an explicit escalation exit (see below).

**Agones** — purpose-built CNCF project for running dedicated game servers on Kubernetes. Provides GameServer lifecycle (Scheduled→Ready→Allocated→Shutdown), dynamic port allocation from a configured range, health checking, fleet autoscaling, and a gRPC/REST allocation API. It sits *underneath* the agent and gives it clean primitives (state, health, a pod to exec into, a defined restart lifecycle) instead of sshing into a box. Adopt it rather than hand-rolling — port allocation, health, and lifecycle are the undifferentiated heavy lifting.

**Edge** — a static UDP+TCP port range (**7000–7010**) forwarded once from the VPS over the existing `wg0` tunnel to the cluster. Lives in `grizzly-platform`, not here. See "Edge forwarding" below.

## Config: two tiers

Keep these separate — they have different authorities and different safety models.

- **Per-game config** (Valheim vs. Minecraft): image, default env, port shape, resource sizing, persistence needs. This is the *catalog* — declarative templates in `games/`, version-controlled, gated, Flux-deployed. Rarely changes. An AI coding agent can author/maintain these at dev time via PR → gate → Flux; that's just normal IaC-with-an-agent.
- **Per-instance config** (this friend's world seed, mods, difficulty, server name): lives on the server's **PVC**, mutated live by the ops agent. It is **not** in any git repo — it's ephemeral friend state, not infra, so it doesn't get PR/gate review. Different tier, different safety model.

## Ops-agent guardrails (non-negotiable)

"Ping me if it breaks, so I'm not fiddling" only holds if these are built in from the start. An unsupervised agent with write access to live game state and restart power can make things worse.

1. **Blast radius.** The agent edits files and restarts pods driven by friends' arbitrary free text (prompt-injection surface). It must be boxed into the game-server namespace — scoped RBAC + NetworkPolicy so a confused or injected agent can't reach the rest of the platform. Given what else runs in this cluster, this is the one that matters most. These guardrails live in `cluster/guardrails/` **in this repo**, so the gate vets them before they deploy.
2. **Snapshot → apply → verify → auto-rollback.** A bad value or mod conflict turns a restart into a crash loop. The agent must snapshot the config dir first, apply, restart, watch health for a bounded window, and revert itself if the server doesn't recover. Without this, "ping me when it breaks" degrades into "the agent broke it worse" — defeating the entire point.
3. **Escalation exit.** Build *apply-known-change* first (bounded, reliable). Give the agent an explicit "I'm not confident — paging Bear" path so open-ended break-fixing escalates instead of thrashing.

## Edge forwarding

Topology (from `grizzly-platform`): VPS `10.200.0.1` ↔ R730xd `10.200.0.2` over `wg0`; R730xd iptables-DNATs ingress ports over the tunnel to a K8s node (`dell_inspiron`, `10.0.0.226`). HTTP/HTTPS (30487/30356) today.

Game forwarding mirrors this: the VPS DNATs the **7000–7010** UDP+TCP range over `wg0` to R730xd, which re-DNATs the range to the K8s node — 1:1, no port remap, so an Agones-allocated port is consistent end-to-end. The range is opened **once**; per-server port assignment happens inside Agones, so the edge never changes per spin-up.

**Node-pin constraint (open):** R730xd forwards the range to a single node IP, but Agones schedules a GameServer pod onto whichever node it picks, exposing a `hostPort` there. So either game-server pods are pinned to the forwarding-target node (`nodeSelector`), or game servers use a NodePort Service (reachable on any node, kube-proxy routes) and the edge targets any one node. **Decision pending** — see Open Decisions.

## Gate + Flux integration

This repo is a gated first-party app (ADR-020 delivery model):

- Ships a root `gate-config.json` honest map; the gate runs on PRs and cosign-signs the images we build; Kyverno `verify-gate-signature` admits them in `grizzly.io/gated=true` namespaces.
- `grizzly-platform` tracks it as a `GitRepository` + `Kustomization` under `kubernetes/apps/grizzly-gameservers/` pointing at this repo's `deploy/` chart. Adding the app is one folder + one list line there; everything else lives here.
- **Standing up Agones is gated here on purpose.** The gate vets external repos before deploy, so security-sensitive standup (the Agones install, the namespace, RBAC, NetworkPolicy) belongs in *this* repo where the gate sees it — not in `grizzly-platform`, which would route it around the check. Co-locating the agent with its own leash means the code and what it's allowed to touch move in one reviewed PR.

**Image admission carve-out:** the gate/cosign/Kyverno path signs *our* images. Agones is third-party and **injects an SDK sidecar into every GameServer pod** (a Google image), so even a signed game-server image shares a pod with an unsigned one. In a `gated=true` namespace Kyverno would bounce the sidecar (and the operator in `agones-system`). So the Agones upstream images need a deliberate carve-out — a scoped Kyverno exception (`cluster/kyverno/`) or keeping the operator out of a gated namespace. The *config* gets gated; the third-party *images* need an explicit admission decision.

## What `grizzly-platform` keeps

Only two things — everything else is here:

- The new Flux source/registration under `kubernetes/apps/grizzly-gameservers/`.
- The edge forwarding (ansible: VPS range DNAT + R730xd range re-DNAT over `wg0`).

## Open decisions

- **Node-pin vs. NodePort** for reaching game-server pods from the edge (see Edge forwarding).
- **Agones packaging** — install as a Helm dependency of the `deploy/` chart vs. a separate gated HelmRelease in the repo. Affects CRD-before-CR ordering.
- **NL front door** — whether/when to add an LLM intent-parser so friends can describe servers in plain English (slash commands cover v1 deterministically; the parser is a bolt-on that must emit *validated* params, never construct k8s objects directly).
- **Per-game catalog format** — how `games/<game>/` expresses image + defaults + port shape + persistence.

## Rejected alternatives (brief)

- **Hetzner Cloud Firewall as the port-management layer** — can't DNAT, so it can't reach home; would duplicate port state. Rejected.
- **frp / rathole relay** — good for the tunnel topology, but once servers are k8s containers the orchestration wants to be k8s-native (Agones), not frp config files.
- **Pterodactyl/Pelican panel** — built for game hosting with a full API, but assumes servers run where its `wings` daemon runs; doesn't map onto the proxy-to-home topology. Bigger lift, real impedance mismatch.
- **Hand-rolled controller instead of Agones** — viable, but rebuilds ~60% of Agones (port allocation, health, lifecycle, allocation API) to avoid a CRD.
