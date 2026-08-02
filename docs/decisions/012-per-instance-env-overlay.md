# ADR-012 — Per-instance environment lives in an overlay file on the PVC

**Status:** Accepted (2026-08-02)

## Context

Friends should be able to ask Gary for mods and datapacks on their Minecraft servers. The platform had no way to do it. Per-instance config is text files on the server's PVC, mutated live through Gary's file tools — and a mod is not a text file. A jar is binary, and a modded Minecraft server additionally needs a loader such as Fabric installed server-side before any jar means anything.

The itzg/minecraft-server image the catalog already runs solves all of that, and solves it entirely through environment variables: `TYPE=FABRIC` installs the loader, `MODRINTH_PROJECTS=<slugs>` downloads mods and datapacks from Modrinth, resolves them against the server's Minecraft version and loader, pulls their required dependencies, and removes artifacts dropped from the list. That resolution pass runs on **every** start of its entrypoint, not only first boot.

So the missing capability was never "download and install mods". It was a **per-instance tier for environment variables**. The catalog tier already sets env per *game*, in git; nothing let one instance differ from another.

## Decision

**A reserved overlay file on the instance's data volume, read by the supervisor immediately before each child launch.** `.grizzly/env`, dotenv-subset format, filtered through an allowlist the *catalog* declares (`SUPERVISOR_CHILD_ENV_FILE` / `SUPERVISOR_CHILD_ENV_ALLOWLIST`), layered between the pod-template environment and the platform-owned injection that always wins.

Three properties made this shape worth the file rather than something more direct.

**It inherits the existing safety machinery instead of adding any.** An overlay edit is an ordinary file write: it produces the standard sibling snapshot, and the existing self-guarding restart loop watches readiness and performs its one bounded auto-rollback exactly as it does for `server.properties`. This design ships **zero** new rollback code. The supervisor seeds the file *empty* at startup and again per launch specifically to keep that true — the verification loop only arms itself for writes that overwrote something, so a file that always exists means even the very first mod install is a snapshotted, rollback-eligible overwrite rather than an exempt creation.

**It applies through an in-place restart.** The supervisor re-reads the overlay when it relaunches the child, so a mod change costs seconds and keeps the pod, the PVC, the Agones allocation, and the friend's address.

**It is game-agnostic.** Nothing about the mechanism knows what Minecraft is; it is the platform's per-instance env tier, and the next game whose tuning surface is env-driven gets it by declaring its own allowlist. The Modrinth-specific knowledge lives in the catalog's five-key allowlist and in Gary's prompt guidance, not in the supervisor.

### Rejected: per-instance env stored bot-side, applied at pod render

A database table consulted when the pod spec is rendered was the obvious alternative. It fails on all three counts above: an env change would require full pod recreation rather than an in-place restart; it creates a second home for per-instance config, splitting the established two-tier model; and it would need its own snapshot, restore, and verification machinery, since none of the file-based guardrails would apply. It also loses archive/restore for free — the overlay rides the whole-volume tarball and comes back byte-identical with its snapshot lineage intact, which was verified rather than assumed.

### Rejected: Gary manages jars directly

A supervisor route that downloads a URL to a path, with the model picking files. This reimplements — worse, and inside an LLM loop — the dependency and version resolution itzg already performs; it requires a new binary-write surface on the supervisor; and it turns "what is installed" into scattered filesystem state instead of one declarative line that can be read, diffed, snapshotted, and rolled back.

### Rejected: the allowlist in the supervisor

Which env keys are safe to expose per-instance is per-*game* knowledge — exactly what the catalog tier exists to hold, and it ships through git review like every other catalog change. Putting it in the supervisor would make a policy decision an image rebuild.

### Rejected: direct-URL and CurseForge install paths

itzg's `MODS`, `PLUGINS`, `DATAPACKS`, and `MODS_FILE` variables are deliberately off the allowlist. Every installed artifact comes from Modrinth by slug, so provenance is always a moderated registry rather than an arbitrary URL an LLM was talked into. CurseForge needs an API key and has messier redistribution terms for marginal catalog gain at friends-scale.

## Consequences

- The platform gained a per-instance env tier, not a mod feature. `.grizzly/` is now the reserved home for platform-owned per-instance state generally.
- Mod resolution re-queries Modrinth at every server start, because that is upstream's behavior. A Modrinth outage degrades modded-server starts. Acceptable at this scale; version-pinned entries reduce the surface.
- The overlay is plainly visible and editable through Gary's general file tools, outside the curated workflow. Acceptable: the allowlist bounds what the file can do, and an edit through any path still gets the snapshot and the rollback.
- Removal is asymmetric in a way no guardrail catches. Setting `MODRINTH_PROJECTS` to an empty value triggers upstream's cleanup pass, but *deleting the line* makes it skip reconciliation entirely — the artifact stays installed while the file claims otherwise. There is no failure to notice, so this is prompt-only discipline in Gary's manager guidance.
- The recreation-loop case [ADR-010](010-recoverable-crash-escalation.md) leaves open is narrowed, not closed: a bad overlay value that nothing rolls back is still re-read by a recreated pod off the same PVC. `mod_info`'s pre-write slug check is the mitigation for the known instance of it.
- `VERSION` is now pinned in the Minecraft catalog rather than resolving to `LATEST` each boot. Without that floor an upstream release would silently move under a validated mod set on the next restart.
