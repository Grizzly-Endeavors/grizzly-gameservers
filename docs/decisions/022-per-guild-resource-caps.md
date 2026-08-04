# ADR-022 — Per-guild resource caps, measured live and backstopped by a namespace quota

**Status:** Accepted (2026-08-04)

## Context

The beta is opening to strangers. Onboarding stays gated behind the operator, but "a guild the operator admitted" is no longer the same thing as "a guild whose members are trusted with the hardware." Before this decision there was no per-guild bound anywhere: the only capacity ceiling was the shared edge port band, first-come-first-served, which one guild could exhaust for everyone — and a stopped server keeps its port, so the exhaustion wouldn't even show up as load. The per-instance clamp (ADR-021) bounds one runaway resize, but nothing bounded the aggregate: N servers × 16 cores / 32Gi each was legal, and N was unbounded.

The goal is explicitly *not* metering. The beta exists to gather real usage data, so the caps must be generous enough that no honest guild ever meets them — they exist to bound the damage of the dishonest or accidental case.

## Decision

**Two caps, both per-guild, both generous.** A **server-count cap** counting every existing instance — running, paused, or stopped, because each holds a NodePort lease and a PVC regardless of process state. And an **aggregate CPU/memory cap** over the *declared limits* of the guild's live pods — limits rather than requests, because limits are the blast radius a guild can inflict on the hardware while requests are the scheduler's concern; declared rather than actual, because the cap must hold at admission time, before a workload has had the chance to misbehave.

**Usage is measured live off the cluster, never from a database.** "Servers owned by guild X" is a label-selector list (ADR-006 made the guild a k8s label); the aggregate reuses the exact `declared_resources` arithmetic the resource sampler records into `resource_samples`, so the enforcement plane and the measurement plane cannot disagree. There is deliberately no `servers` table: the cluster is already the source of truth, and a registry would be one more thing to drift.

**Caps are env-set global defaults plus per-guild overrides in Postgres.** `GAMESERVERS_GUILD_MAX_SERVERS` / `_MAX_CPU` / `_MAX_MEMORY` default to **5 servers / 24 cores / 48Gi** — three simultaneous instances of the heaviest catalog game (Satisfactory, 8c/16Gi limits), or one server at the per-instance ceiling plus a big game beside it, while staying under half the cluster's workers. The `guild_limits` table holds per-column nullable overrides (`NULL` = use the default), so the operator can raise just one axis for one guild. The façade follows the `InstanceSizing` shape — but with the opposite degradation: **Postgres down means the global defaults apply, not a refusal.** A sizing override *records* state that would silently evaporate; a cap override only *reads* policy, and the generous baseline is a safe answer. Enforcement therefore never turns off.

**Enforcement points, chosen so every path is covered by construction:**

- **Create and recover** — inside `provision_under_lock`, under the same provision lock that serializes port leasing, so two concurrent creates can't both squeeze under the cap. Both `/create` and Gary's `create_server` funnel there, and recover does too — checked against the **archive's own owning guild** (the guild the server returns to), not the caller's. Callers resolve caps and pass them in (`ProvisionSpec.caps`), keeping `agones/` free of a Postgres dependency; `None` means the caps don't apply (an operator's unscoped instance). Per ADR-014, events are recorded by the executing layers.
- **Resize** — in `exec_set_resources`, the only resize entry point, which also covers the watchdog's unattended manager-tier turns. The cap binds the server's *owning* guild, read off its Service label rather than the channel the request came from, so an operator DM resizing a guild's server hits that guild's bound. Usage is measured with the target excluded, so a proposal replaces the target's current contribution instead of stacking onto it.
- **Cold start** — in the two user-driven shells (`/start`'s cold branch and Gary's cold-start path), via one shared gate, held together with `begin_start` under the provision lock. Without this, the aggregate cap is trivially escaped: create N servers while others are stopped (stopped pods contribute nothing at create time), then start everything.

**The start-path asymmetry is deliberate.** A *warm* resume is not gated — its pod never went away, so its load never left the aggregate. The backup cycle's ensure-pod and the stranded-instance reconciler are also not gated: both restore previously-admitted load, and refusing the ensure-pod path would trade a world's data safety for quota purity. Only the pod-*recreating*, user-driven start adds new load, and only it is checked.

**Refusals carry their figures everywhere.** `LimitRefusal` names current, adding, and cap; embeds, Gary tool results, and the event log all render from it. The event outcomes are `guild_limit_servers` / `guild_limit_resources` on the existing action kinds — policy capacity, kept separate from `ports_exhausted` (physical capacity) in `capacity-rejections.sql`, because each calls for a different response: raise a cap versus buy ports.

**Adjustment is operator-only, and visibility is admin.** `/config limits view` shows any admin the caps their guild is under and its live usage against them; the mutation subcommands require the cross-guild operator (`require_operator`). `require_admin` is exactly the wrong gate here — guild admins are the party the caps bind, and the tier that grants everything *inside* a guild must not reach the knobs that bound the guild itself. Not a Gary tool: that would need a fourth access tier plus prompt surface for a rare human governance action.

**A namespace `ResourceQuota` backstops the bot-side checks** (`cluster/guardrails/resource-quota.yaml`): a generous total-namespace ceiling on requests/limits/PVCs that catches any bug in bot-side enforcement. It is global, not per-guild — defence in depth, not the policy. ADR-021's rejection of a `LimitRange` stands (it would impose values on the Agones sidecar); a quota only requires that declarations *exist*, which is why the Agones SDK sidecar must be given explicit resource values in the Agones Helm release **before** the quota lands — an undeclared sidecar container would otherwise make admission reject every new game pod. The rollout is sequenced accordingly: bot-side caps, then sidecar declarations (verified on a live pod), then the quota.

## Consequences

- No single guild can exhaust the port band, the PVC pool, or the workers' CPU/RAM, however it drives the bot — slash commands, Gary, deferred batches, and the watchdog all pass through the same three gates.
- The cap arithmetic being the sampler's arithmetic means `resource_samples` can always explain a refusal after the fact.
- Cap refusals are now a queryable demand signal: sustained `guild_limit_*` outcomes for a guild are the data-driven case for raising its override — or the early warning that the beta needs more hardware.
- The whole-pod aggregate includes the sidecar's declared limits once Branch B lands, while a newcomer's "adding" contribution is its game container's catalog sizing — a per-pod skew of one sidecar (~100m/64Mi), acceptable inside caps this generous.
- An unreadable catalog template contributes zero to the aggregate rather than refusing the create — a catalog bug must not become a guild-wide outage. The count cap still holds regardless.
- Two k8s list calls (Services, pods) run under the provision lock per create; at friends-scale traffic this is noise, but it is the first real work done under that lock.

## Alternatives rejected

- **A `servers` registry table in Postgres.** The cluster already knows; a registry drifts and needs reconciling. Live label-selector lists are one RTT and always true.
- **An admission webhook enforcing per-guild policy.** The k8s-native answer and massively over-scoped for one bot in one namespace — a webhook is an availability liability (fail-open or block the namespace) and moves policy away from the code that can phrase a refusal for a non-technical friend.
- **Per-instance clamp alone (status quo).** Bounds one server, not a guild; N × ceiling was legal.
- **Counting only running servers.** Simpler story, but a guild could hoard ports and PVCs by stopping servers — the two resources a stopped server still holds are exactly the scarce ones.
- **Caps on actual usage rather than declared limits.** Actuals arrive after the damage starts and fluctuate with player load; declared limits are what admission can hold and what the node genuinely risks.
- **Refusing creates when the catalog sizing can't be read.** Fails closed on the wrong party: a template typo would lock every guild out of that game entirely, with the count cap still bounding the abuse case.
