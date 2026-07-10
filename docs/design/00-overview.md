# grizzly-gameservers — High-Level Design

**Status:** Design of record. The system described here is implemented and deployed — see `docs/activation-status.md` for current live/deferred status.

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
   │                           │  (via the per-pod supervisor's HTTP control API)
   ▼                           ▼
Agones (lifecycle, ports, allocation API, health)
   ▼
GameServer pods (per-game image + per-instance config on a PVC)
   ▲
   │  static port range forwarded once
Hetzner VPS edge ──wg0 tunnel──► R730xd ──DNAT──► K8s node
```

### Components

**Discord shim** — the only piece that is purely "our product." Thin: authenticate the friend, map a command to an Agones allocation/teardown, hand back `IP:port`. No quotas, no billing — this is a friends-scale service.

**Ops agent** ("Gary") — the center of gravity. An LLM loop (mentioned in Discord) that translates intent and operates running servers: read current config + logs, decide the change, mutate the right file, restart the GameServer, watch health, report back — all via the per-pod supervisor's HTTP control API (see below), never `kubectl exec` (see "Why not exec-as-tools" in [`01-sidecar-agent-interface.md`](01-sidecar-agent-interface.md)). This is what replaces N hand-rolled per-game config managers. Two distinct uses, only one of which is autonomous:
- *Apply a known change* (gather rate, difficulty, add a mod) — bounded and reliable. Shipped first.
- *Diagnose an arbitrary failure and fix it* — open-ended; the agent will sometimes flail here. Scope it with an explicit escalation exit (see below).

**Agones** — purpose-built CNCF project for running dedicated game servers on Kubernetes. Provides GameServer lifecycle (Scheduled→Ready→Allocated→Shutdown), dynamic port allocation from a configured range, health checking, fleet autoscaling, and a gRPC/REST allocation API. It sits *underneath* the agent and gives it clean primitives (state, health, a defined restart lifecycle) instead of sshing into a box. Adopt it rather than hand-rolling — port allocation, health, and lifecycle are the undifferentiated heavy lifting.

**Per-pod supervisor** — a thin Rust binary (`grizzly-supervisor`) baked into each game image as its entrypoint, launching the game server as a child process. It owns the Agones SDK heartbeat (so it replaces a readiness sidecar) and serves an in-pod HTTP control API the bot/ops agent drive to stop/start/restart the game *process in place* — the pod, PVC and Agones allocation survive, so a restart is seconds rather than a reschedule. `/stop` pauses the process (pod stays up); `/shutdown` is the heavier teardown that deletes the GameServer. This is also the substrate the ops agent's file tools (browse/read/write/restore) and RCON console bridge (`send_command`) dock into — all shipped. Design: [`01-sidecar-agent-interface.md`](01-sidecar-agent-interface.md).

**Edge** — a static UDP+TCP port range (**7000–7010**) forwarded once from the VPS over the existing `wg0` tunnel to the cluster. Lives in `grizzly-platform`, not here. See "Edge forwarding" below.

## Config: two tiers

Keep these separate — they have different authorities and different safety models.

- **Per-game config** (Valheim vs. Minecraft): image, default env, port shape, resource sizing, persistence needs. This is the *catalog* — declarative templates in `games/`, version-controlled, gated, Flux-deployed. Rarely changes. An AI coding agent can author/maintain these at dev time via PR → gate → Flux; that's just normal IaC-with-an-agent.
- **Per-instance config** (this friend's world seed, mods, difficulty, server name): lives on the server's **PVC**, mutated live by the ops agent. It is **not** in any git repo — it's ephemeral friend state, not infra, so it doesn't get PR/gate review. Different tier, different safety model. Because this tier is un-versioned and lost on `/destroy`, it's the tier the **backup/archive/restore** feature preserves: durable off-cluster snapshots to S3, an archive that frees the PVC while keeping the world recoverable, and restore/recover to bring it back. Design: [`02-backups-archive-restore.md`](02-backups-archive-restore.md).

Distinct from both is **Gary's memory** — durable operational *knowledge* he learns while running a game (e.g. "Palworld must be soft-stopped before a config edit applies"), not config. A session's conversation is ephemeral, so without this Gary rediscovers each game's quirks every time. Facts are scoped to a catalog game id (or a `general` bucket), **shared across every guild** (a quirk learned on one community's server helps them all — the same "absorb each game's idiosyncrasy generically" reason the agent exists), and **self-authored at runtime**: Gary saves them via a manager-tier `remember` tool and they're injected back into his system prompt each session; `forget` (and an admin `/gary-memory` command) prunes them. They live on the foundation Postgres store, not git — the live tier, like per-instance config, since Gary has no repo access and the value is immediate. Same graceful degradation as the other Postgres-backed state: no DB, no memory, bot otherwise unaffected.

## Tenancy: servers are scoped to a Discord guild

One Gary can serve more than one friend group — Bear's friends in one Discord server (guild), his Dad's friends in another — without either group seeing the other's servers. The **tenant boundary is the Discord guild id**. Every server is stamped, at create time, with the guild it was born in (the `…/guild` label on its GameServer/Service/PVC trio); that label rides on the surviving Service, so scope persists across a stop/start. Then every read and every action confines itself to the caller's guild: `/servers`, autocomplete, Gary's `list_servers`, and every server-targeting command or tool. Within a guild, servers are **shared across every channel** — the channel you run a command in doesn't matter. A server in another guild reads back identically to one that doesn't exist, so scoping never leaks another group's servers. (This replaced an earlier per-*channel* boundary — a dev-era shortcut — that made archive/recover disagree on which channel owned a server; see [ADR-006](../decisions/006-guild-tenancy.md).)

The one exception is the **operator**: a user id on the `GAMESERVERS_ADMIN_USER_IDS` allowlist sees and manages every guild's servers, and can act even from a DM. This cross-guild view is deliberately keyed on the *user-id allowlist only* — a guild's own admins can manage their guild but can't reach another's.

**Who can act** in a guild is graded into three tiers, computed by one pure policy function (`access_level` in `discord/auth.rs`) that both the slash-command checks and Gary's tool selection call through:

- **Admin** — full control, including the destructive and governance actions (`/destroy`, `/archive`, `/restore`, `/recover`, `/config`, `/gary-home`, `/gary-memory`, and Gary's console commands). Admins are the union of the cross-guild operators, the guild **owner**, and the admin **roles/users configured with `/config`**. The owner is the bootstrap path — a fresh guild is usable via `/config` before any env change.
- **Manager** — day-to-day server operation: the lifecycle (`/create`, `/start`, `/stop`, `/restart`, `/shutdown`) and `/backup`, plus asking Gary to inspect and *edit* config files for troubleshooting and to `remember`/`forget` operational facts about a game. A manager **cannot** delete a server, overwrite a world, or change who has access. Granted per-guild with `/config manager-role` / `/config manager-user`. `Admin` implies every `Manager` privilege.
- **Read-only** (everyone else) — lookups only: `/servers`, `/backups`, `/archives`, and Gary's read-only tools.

Grants are stored per-guild in Postgres (a roles table and a users table per tier). Auth degrades **fail-closed**: if the config DB is down, only operators and the owner are recognized (as admins), and everyone else — including would-be managers — falls back to read-only. A **DM** has no guild, so a non-operator asking Gary to act there is refused with guidance to use a guild channel; operators keep their all-guilds view in DMs.

Enforcement lives at two choke points so it can't be forgotten as commands/tools are added: poise `check`s (`require_admin` / `require_manager`) gate each mutating slash command, a single poise `command_check` gates every slash command carrying a `server` argument, and Gary's tool `dispatch` gates every tool that names an existing server on the caller's tier. Because the bot is multi-guild, slash commands register **per guild** on the `GuildCreate` event (instant on join; no ~1h global-propagation wait). Port allocation and name-clash checks stay namespace-global (ports are a shared cluster resource; object names must be unique regardless of guild).

## Talking to Gary: mentions, DMs, and home channels

By default Gary answers when `@mentioned`. That's a deliberate floor, not just a UX choice: without Discord's privileged **Message Content intent** a bot only receives message *content* for messages that mention it or are DMs to it, so a mention is the one thing that always arrives. With the intent enabled, Gary can also treat a whole channel as his own:

- **`@mention`** — works anywhere, always. The historical default.
- **DM** — every DM to the bot is answered without a mention (DM content is always delivered).
- **Home channel** — an admin runs **`/gary-home`** to toggle the current channel; there, Gary answers every message with no mention needed. This is what lets one Gary be dropped into a friend group's channel and "just talk." Note home is purely about *no-mention answering* — it is **not** the tenant boundary. Which servers exist is a property of the whole [guild](#tenancy-servers-are-scoped-to-a-discord-guild), shared across all its channels; homing Gary in a channel doesn't move, hide, or scope any server. (Conflating the two is exactly the confusion the guild-tenancy change removed.)

In the no-mention paths, blank lines and slash-command-style text (leading `/`) are ignored, so Gary doesn't chime in on every stray message. The home-channel registry is one of the bot's several pieces of **durable state** on the foundation Postgres store (a role-owned DB, provisioned out of `grizzly-platform`) — alongside the per-guild access config, the archive index, and Gary's memory; it's cached in memory so the per-message check is free. Persistence **degrades gracefully** — if Postgres is unreachable, mentions and slash commands keep working and only no-mention home channels go dark until reconnect.

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

**Image admission carve-out:** the gate/cosign/Kyverno path signs *our* images. Agones is third-party and **injects an SDK sidecar into every GameServer pod** (a Google image), so even a signed game-server image shares a pod with an unsigned one. In a `gated=true` namespace Kyverno would bounce the sidecar (and the operator in `agones-system`). Resolved for the current phase: enforcement is bot-scoped only, so the sidecar and game/supervisor images (not yet gate-signed in CI) are untouched by policy — full-namespace enforcement stays a documented follow-up ([ADR-003](../decisions/003-bot-scoped-gate-enforcement.md)).

## What `grizzly-platform` keeps

Only two things — everything else is here:

- The new Flux source/registration under `kubernetes/apps/grizzly-gameservers/`.
- The edge forwarding (ansible: VPS range DNAT + R730xd range re-DNAT over `wg0`).

## Open decisions

- ~~**Node-pin vs. NodePort**~~ — resolved: NodePort, no node-pinning ([ADR-002](../decisions/002-nodeport-no-node-pin.md)).
- ~~**Agones packaging**~~ — resolved: standalone gated HelmRelease ([ADR-001](../decisions/001-agones-packaging.md)).
- ~~**NL front door**~~ — resolved: Gary, the `@mention`-triggered tool-calling LLM agent (`crates/bot/src/discord/gary/`), sits alongside the deterministic slash commands rather than replacing them.
- ~~**Image admission carve-out**~~ — resolved for the current phase: bot-scoped gate enforcement ([ADR-003](../decisions/003-bot-scoped-gate-enforcement.md)); full-namespace enforcement is a documented follow-up.
- **Per-game catalog format** — how `games/<game>/` expresses image + defaults + port shape + persistence. Seeded by `games/minecraft/`; not yet generalized.

## Rejected alternatives (brief)

- **Hetzner Cloud Firewall as the port-management layer** — can't DNAT, so it can't reach home; would duplicate port state. Rejected.
- **frp / rathole relay** — good for the tunnel topology, but once servers are k8s containers the orchestration wants to be k8s-native (Agones), not frp config files.
- **Pterodactyl/Pelican panel** — built for game hosting with a full API, but assumes servers run where its `wings` daemon runs; doesn't map onto the proxy-to-home topology. Bigger lift, real impedance mismatch.
- **Hand-rolled controller instead of Agones** — viable, but rebuilds ~60% of Agones (port allocation, health, lifecycle, allocation API) to avoid a CRD.
