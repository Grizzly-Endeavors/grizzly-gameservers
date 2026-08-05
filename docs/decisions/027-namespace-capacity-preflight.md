# ADR-027 — Price a create against the namespace quota before leasing anything

**Status:** Accepted (2026-08-05)

## Context

ADR-022 gave every guild a cap and put a namespace `ResourceQuota` behind it as a backstop. The caps work. The backstop did not — not because it failed to hold the line, but because of *when* Kubernetes enforces it and what the friend was told when it did.

A quota is enforced at admission. For a PVC that is immediate, so an exhausted `persistentvolumeclaims` ceiling came back as a `403` from the PVC create, surfaced as `Err`, and rendered as **"Couldn't create the server right now. Try again in a moment."** — a permanent capacity wall described as a transient blip, to an audience explicitly told retrying would help. Strangers retry-loop against that.

The CPU/memory case was worse. Custom resources aren't quota'd, so the `GameServer` create *succeeded*; Agones then tried to create the pod and *that* was rejected. Nothing on the create path ever saw an error. The friend got an address, then a five-minute readiness wait, then "still starting" — while the instance held a leased edge port, a PVC, and a `GameServer` object for a pod that was never going to be admitted.

Two smaller problems came with it. The quota's own comment named the 100-port edge band as the binding constraint, but 16 PVCs at a 5-server guild cap meant the real ceiling was about three guilds, and the arithmetic didn't match the caps it was supposed to backstop. And the bot's own Deployment lives in `game-servers` with default rolling-update surge, so a rollout needs a second pod's worth of room *on top of* the running one — while quota is first-come-first-served and game pods are created continuously. Game pods can make the replacement bot pod inadmissible and stall a rollout that CI and Flux both report as successful.

## Decision

**The bot reads the namespace quota and prices each admission against it before leasing anything.** `crate::capacity` holds the arithmetic — a `NamespaceQuota` of hard-and-used pairs, an `Admission` of what one instance adds, and `check_admission` returning the first dimension that can't take it. `agones::capacity` reads the live `ResourceQuota` and prices an instance from its catalog template. The check runs inside `provision_under_lock`, immediately after the guild-cap check and under the same lock, and again on the cold-start path where a pod is recreated.

**The `ResourceQuota` stays the single source of truth.** The bot mirrors none of its figures; it reads the object admission actually enforces. Where several quotas cover the namespace, the tightest room per dimension wins, because a pod must satisfy all of them.

**An unreadable quota refuses nothing.** No quota object, or RBAC that won't allow the read, yields an empty snapshot that admits everything. A bot that cannot see the ceiling must not invent one — admission still holds the line, the friend just gets the worse message. That is strictly the old behaviour, so the failure mode of this feature is the absence of the feature.

**The estimate includes the Agones SDK sidecar.** It appears in no catalog template but the quota prices the whole pod, so leaving it out is precisely how an estimate passes here and is rejected at admission anyway. Its figures mirror `cluster/agones/helmrelease.yaml`.

**A live rejection is still handled, and still means "full".** A `403` carrying `exceeded quota` on the PVC or `GameServer` create rolls the instance back and reports the same refusal, not an error. The pre-flight races the quota controller's own accounting and models only the dimensions a game pod is known to consume, so the rejection is the authoritative answer; `CapacityRefusal::Unspecified` covers the case where a re-read no longer says which dimension bound.

**One bot pod's worth of headroom is withheld from every game admission.** This is the rollout-stall fix, and it is deliberately bot-side policy rather than a second quota or a move to another namespace. A separate quota scope would need the bot's pods labelled and both objects kept in step; moving the bot out of `game-servers` would break the NetworkPolicy topology that the game pods' egress leash and the panel's ingress policy are both built on. Withholding the last slice costs one server's worth of capacity at the very top of the range and needs nothing outside this repo. Its figures mirror the chart's `bot.resources`.

**CPU and memory are the binding constraint; the PVC count is raised out of the way.** `persistentvolumeclaims` goes 16 → 32 and `requests.storage` 500Gi → 600Gi. Thirty-two claims is six guilds' worth of instances at the default 5-server cap, well past what ~54 usable cores and ~118Gi can host — so the wall a guild meets is CPU or memory, denominated in exactly the figures their own cap is written in. Storage is looser than the PVC count only for the largest catalog game, and that case is reported as storage.

**Guild count stays uncapped.** `GAMESERVERS_GUILD_ALLOWLIST` is already the admission gate for who is served at all, and it is operated by hand. A numeric guild cap would duplicate it with worse ergonomics — the operator is watching real usage and gating new guilds himself.

**Refusals carry their figures, and read differently from a guild cap.** A cap is the community's own allowance and somebody in the room can free it by stopping a server; this is the whole service being out of room, which mostly only the operator can fix. Both the embed and Gary's tool results say so explicitly, including that waiting won't help — that sentence is the actual fix for the reported bug. The event log records `cluster_full_*` outcomes split by dimension and, for CPU/memory, by whether requests or limits bound, because those are different things to raise.

## Consequences

- Two extra API reads on the create path (the quota list, then the guild usage that was already there). Both are namespace-scoped list calls at friends-scale volume.
- The estimate can be conservative: the sidecar figures and the bot reserve are compiled constants mirroring cluster config, and a drift in either direction only changes which honest message a friend gets — never whether the cluster admits the pod.
- The readiness timeout remains the backstop for a genuine race, but it is no longer the common path for a full namespace.
- The bot now needs `get`/`list` on `resourcequotas`. Read-only, namespace-scoped, and degrading to today's behaviour without it.
- Raising the PVC ceiling assumes headroom on the iSCSI ZFS pool for the additional claims. The quota bounds what may be requested, not what exists; the pool is not managed from this repo.
