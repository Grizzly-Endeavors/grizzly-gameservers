# ADR-009 — Recover stranded instances by force-deleting the pod, not tainting the node

**Status:** Accepted (2026-07-27)

## Context

A power outage on 2026-07-24 took one worker node down for two and a half days. A Palworld world running on it stayed down for the entire outage, even though two other nodes had ample capacity and the world lives on network block storage with no node affinity. Nothing about the infrastructure required it to stay put.

Three things combined to pin it there.

**Kubernetes couldn't finish evicting the pod.** Taint-based eviction deleted it about five minutes after the node stopped answering, but a delete only completes once a kubelet confirms the kill. With the node off, the pod hung in `Terminating` for the whole outage, still reporting `Running` with its old pod IP.

**Agones wouldn't release the `GameServer`.** `syncGameServerDeletionTimestamp` is explicit — *"but no removing finalizers until it's truly gone"* — so a `GameServer` whose pod still exists keeps its `agones.dev/controller` finalizer and stays in the API. Nothing recreates a standalone `GameServer` either: there is no Fleet, so no controller had "keep this running somewhere" as its job.

**The bot read the tombstone as a live server.** `instance_runtime_state` decided warm-vs-cold on whether the `GameServer` object existed, so `/start` chose the warm path and dialled the dead pod's IP. `/shutdown` didn't help: `delete_if_present` treated the accepted delete as completion, reported the world "fully shut down and saved", and left the object exactly where it was. The warm path needed a live pod, the cold path needed the `GameServer` gone, and the `GameServer` couldn't go until the node returned. There was no exit.

Two facts about the volume are worth recording, because both were suspected and neither was the cause. The PV is `democratic-csi` iSCSI with empty `nodeAffinity` — genuinely portable. And `--disable-force-detach-on-timeout` is unset on this cluster's kube-controller-manager (it defaults false), so the attach/detach controller will force-detach from an unhealthy node once the pod is gone. Storage was never what held the world in place.

## Decision

Deletes that a caller depends on must be **confirmed**, not merely accepted. `delete_and_confirm` polls until the object has actually left the API; `deletion_state` classifies an object's lifecycle from its metadata. `/shutdown` reports `Stranded` rather than claiming success, `/destroy` refuses to touch the Service or PVC when the `GameServer` won't confirm, and `RuntimeState::Terminating` never routes to the warm path.

A **reconciler in the bot** sweeps for pods that have been terminating longer than a dwell window (15 minutes by default, well clear of the game pods' 120s graceful-stop) and **force-deletes them with `grace_period_seconds: 0`**. That single act lets Agones release the finalizer, after which the instance is either recycled onto a healthy node or left down.

Recycle-or-release is derived from cluster state rather than stored: a `GameServer` still intact when its pod stranded means the instance was running and its node died, so it comes back; one already carrying a `deletionTimestamp` means a shutdown was asked for, so the teardown is allowed to finish. No desired-state annotation is needed, and no `services: patch` grant with it.

Recovery is **silent to Discord**. Friends can't act on "your node is down", so it logs at `warn`/`info` and nothing more.

### Rejected: taint the node `node.kubernetes.io/out-of-service`

This is the Kubernetes-sanctioned answer to a non-graceful node shutdown, and it is strictly better mechanics — it force-deletes the node's pods *and* detaches their volumes properly. It is rejected because it needs cluster-scoped `nodes` patch, and `docs/design/00-overview.md` calls confining this workload to its own namespace "the one that matters most": the bot is also Gary's identity, and Gary acts on friends' arbitrary free text. Trading a namespace-scoped blast radius for a cluster-scoped one to save a single pod delete is the wrong trade. The taint stays a documented operator step in `grizzly-platform`'s cluster-ops runbook.

### Rejected: detect stranding by reading node conditions

Checking whether a pod's node is `NotReady` is the direct signal, but `nodes` is a cluster-scoped resource, so even read access means a `ClusterRole`. A pod still terminating long past its grace period carries the same information — a kubelet that is answering finishes a delete inside that window — and needs only the `pods` access the bot already had.

### Rejected: switch game PVCs to `iscsi-zfs-retain`

Retain-policy PVs would have made a bad `/destroy` survivable, but they leave an orphaned PV behind after every legitimate destroy. Hardening `/destroy` to refuse an unconfirmed teardown removes the failure at its source instead of catching it downstream.

## Consequences

- The bot's `Role` gains `pods: delete`, its first grant that mutates a pod. It is still namespaced, still has no `patch` and no `exec`, and still never touches nodes. `cluster/guardrails/bot-rbac.yaml` and its README record why.
- Force-deleting a pod on a node that is merely *partitioned* rather than dead could in principle let two writers touch one RWO volume. The dwell window makes this unlikely, and the alternative — leaving worlds pinned to dead hardware until a human notices — is the failure that actually happened.
- A `GameServer` that stays held after its pod is forced out is left for the next pass rather than recreated, so a stuck finalizer can't produce two objects competing for one name.
- `/shutdown` now takes as long as the world-save actually takes (up to 150s) before it answers, because it waits for the truth instead of reporting the request.
- Recovery is invisible in Discord by design. The reconciler's logs are the only record that it ran, so a recycle loop would show up in logs rather than as chat noise.
