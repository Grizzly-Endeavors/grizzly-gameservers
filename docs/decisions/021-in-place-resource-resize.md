# ADR-021 — Resize a running server in place; keep the override in Postgres

**Status:** Accepted (2026-08-04)

## Context

A server's CPU and memory were a per-game catalog decision (`games/<game>/gameserver.yaml`), identical for every instance of that game, and nothing in the bot had ever changed one. Every other tier of config had a live path — files on the PVC, the env overlay, the console — but the size of the box the game runs in did not.

That gap sat directly under a measurement the product plane was built for. ADR-015 records CPU quantities "for packing and right-sizing analysis"; `docs/archive/data-collection/design.md` says RAM reclaimed by right-sizing is "explicitly deferred: Gary has no right-sizing capability today, so there is nothing to measure yet." The watchdog's memory-pressure incident (ADR-016) detects a server pressing against its limit and its prompt then told Gary the limit "is not yours to change" — the one signal most obviously actionable, and the one thing he couldn't act on.

The obvious implementation — record a new size, recreate the pod — makes the fix cost what the problem cost: everyone playing gets disconnected. That is the wrong trade for the case that matters most, a server about to be OOM-killed *while people are on it*.

## Decision

**Resize in place, through the `pods/resize` subresource.** The cluster runs Kubernetes 1.33 with in-place vertical scaling on (verified: game pods already publish `status.containerStatuses[].allocatedResources`). A patch there changes a running container's CPU and memory with no restart, no reschedule, and nobody disconnected. That property is the reason this is a tool at all rather than a `shutdown`/`start` pair, and both prompts state it — a model that doesn't know it reaches for `run_when` and makes a friend wait for an empty server for a change that needs no downtime.

**The RBAC grant is the subresource, not the object.** `cluster/guardrails/bot-rbac.yaml` states in three places that the bot never patches — every lifecycle path replaces whole objects. This is the first exception and it is deliberately the narrowest available shape: `pods/resize` accepts changes to `spec.containers[*].resources` and nothing else, so the grant cannot reach the image, env, volumes, or security context. `patch` on `pods` would have bought the same feature and a great deal more. The replace-never-mutate rule still holds for every whole object.

**Overrides live in Postgres, not on the Kubernetes object.** `begin_start` re-renders the `GameServer` from the catalog template, so a resize held only in the cluster would silently revert the next time a server was stopped and started — and revert *quietly*, which is the worst kind. `crate::sizing::InstanceSizing` mirrors the `GuildConfig` façade: cached in memory, loaded at startup, disabled rather than fatal when Postgres is down. `render_gameserver` stamps it on at every provision *and* every cold start, and the four `begin_start` callers — including the backup cycle's ensure-pod and the stranded-instance reconciler — all pass it, because a server brought back to be backed up must come back the size it was.

**Unavailable is a refusal, not a fallback.** With no database the tool changes nothing and says so. A resize that lands on the pod but not in the record would work until the next restart and then undo itself; that is worse than not resizing, and it is the same posture `run_when` takes when its queue is unreachable.

**Refuse out-of-range values rather than clamping them.** A floor (250m / 512Mi — below which a game either won't start or thrashes) and a ceiling (16 cores / 32Gi — above which nothing can be scheduled on any node here), plus the request-under-limit rule Kubernetes would otherwise reject as an unschedulable pod. Each rejection names the figure and its bound, so the model can propose something that would work. A friend told "done" who quietly got a different number is worse served than one told the value was out of range.

**Manager tier, with the brake in the prompt and the clamp.** The watchdog runs its unattended turns at manager tier, so admin-gating would have left the memory-pressure incident detecting a problem it still couldn't fix. What bounds runaway allocation is the clamp in `crate::sizing` and a prompt that makes growing the allocation the *last* option: a memory leak, a runaway mod, or a view distance nobody meant to set will consume whatever it is given, and resizing hides that until the bigger limit fills too. Both the tool description and the manager system-prompt block carry the diagnose-first rule, because the tool description is only read once the model has already decided to resize.

**Patch, then watch.** The kubelet applies a resize asynchronously and may decline one it can't action — shrinking a memory limit below live usage, or growing past what the node has left. Neither shows up as an error on the patch. So the tool polls `status.containerStatuses[].resources` (what the container is *running* with, never the spec, which reflects the request the instant it is accepted) against the `PodResizePending` / `PodResizeInProgress` conditions, and reports four distinct outcomes: applied, still settling, infeasible, or recorded-for-next-start. `Deferred` is deliberately not treated as infeasible — it means the node is full *now* and the resize stays queued.

**`MEMORY` joins Minecraft's env allowlist.** The pod's memory ceiling and the JVM heap are separate numbers, and the heap is fixed at launch. An in-place memory raise still helps a Java server — more room for metaspace, direct buffers and region mmaps, and no OOM kill — but it does not give the game more heap. Putting `MEMORY` on the per-instance overlay (ADR-012) makes the heap a value Gary can set and apply with a restart, so the flagship game's memory lever works rather than half-works. Both prompts state the caveat.

## Consequences

- The right-sizing experiment the cost plane was built to run is now possible: a new `resources_changed` event records who changed what and how it landed, beside the `resource_samples` series that already records what each instance was given per tick.
- A destroyed server's override is cleared on both destroy paths. Names get reused, and a stale row would silently size an unrelated future world.
- `set_resources` requires all four figures rather than accepting a partial change, so the model must read `server_resources` first. A partial stamp could pair a new request with an old limit and invert the two.
- Quantities cross the wire as Kubernetes strings ("2Gi", "500m") rather than numbers, matching every other surface a friend or operator sees. Comparison is on parsed values, because the API normalizes what it returns ("2000m" comes back as "2") and a string comparison would report a finished resize as pending forever.
- `run()` outgrew the line cap, so the five Postgres-backed façades became a `DurableState` bundle — they share one credential and one degradation story, which is a better shape than five sequential lines regardless.
- **Storage is not in scope.** `iscsi-zfs` has `allowVolumeExpansion: true`, but growing a PVC needs `patch` on the whole object rather than a narrow subresource, and it is irreversible. Its own decision, when there is a signal to justify it (issue #70 tracks the disk-usage reading that would).

## Alternatives rejected

- **Record the size and apply it by recreating the pod.** Simple, works on any Kubernetes version, and disconnects everyone playing — for the case where the server is under pressure *because* people are playing. The live path is the point.
- **Store the override as an annotation on the instance's Service.** The Service survives a shutdown and already carries the port plan, so it looks like the natural home. But rewriting an annotation on a live Service needs `patch` on `services` — a whole-object grant, wider than the resize subresource, bought for the *storage* half of a feature whose apply half already needed nothing.
- **Admin-only.** Cleaner-sounding, and it would have kept the watchdog reporting a problem it could not fix. The tier was the wrong lever; the clamp and the prompt are the right ones.
- **Clamp out-of-range values into the allowed band.** Always "succeeds", and silently gives a friend something other than what they asked for.
- **A namespace `LimitRange` as the bound.** Admission-enforced and appealing, but it would apply to the bot's own pod and the Agones sidecars too, and it produces a rejection with no useful message for a non-technical friend. Bot-side validation names the figure and the bound; a `LimitRange` can still be added underneath as defence in depth.
