# ADR-035 — A crashed launch's output is preserved before anything can overwrite it

**Status:** Accepted (2026-09-09)

## Context

[ADR-034](034-agent-read-only-shell.md) gave Gary a shell, which fixed his inability to *search* a server. It left the other half of the original defect standing: the evidence he would search is destroyed within seconds of the crash that produced it.

`LogBuffer` is the only log source that exists for every game, and it is a 1000-line in-memory ring created once per supervisor and shared across every child launch. Three things then eat a crash:

- The supervisor relaunches a crashing child several times before escalating, stacking failed boots on top of one another in the same ring.
- The bot's auto-rollback restores the config and restarts — and one healthy Valheim boot is 3737 lines, so that single restart overwrites the whole ring more than three times over.
- If escalation reaches an Agones `Unhealthy`, the pod is recreated and the ring is gone entirely.

The rollback is the sharpest case, and the irony is the point: the guardrail that saves the world is the thing that destroys the only account of what went wrong. Gary reports that he rolled a change back and cannot say why he had to.

The games' own logs don't close this. Minecraft's rotated `logs/` and timestamped `crash-reports/` are on the PVC and do survive — but Valheim's BepInEx log is on the container's writable layer, outside the data root, and is truncated at every boot (`AppendLog = false`). What is uniform across all six games is the child's stdout, which is what the supervisor already captures.

## Decision

**Each launch's output is captured separately and written to the instance's volume when that launch dies unexpectedly.** The volume outlives the pod, the file outlives the next launch, and the agent reads it with the file tools or greps it with the shell.

Artifacts land in `.grizzly/crashes/` — the reserved tree the supervisor owns and the agent reads, alongside the env overlay and the mod list — named by timestamp, with the five most recent retained.

**Head and tail, not a tail.** The two crash shapes put their evidence at opposite ends: a mod that fails to load throws during startup, because BepInEx chainloads before the world is up; a server that dies after a week says why in its last few lines. A plain ring keeps only the second. So the capture keeps the first 3000 lines and the last 12000, and states how many it dropped between them. The head budget is deliberately larger than a full modded boot, so the case that actually motivated this — a startup failure — is preserved whole with nothing elided at all.

That sizing is what makes "just save the whole file" true in practice while keeping memory bounded for a server that runs for a month before dying.

### The pumps have to drain first

A child's exit and the arrival of its final lines are two different events, and the final lines are the ones that say why it died. So the capture tracks how many output pumps are still reading and waits, briefly, for them to finish before writing. A wait that times out still writes the artifact and logs that its tail may be short — a slightly incomplete record beats none.

### Ordering by mtime, and never reusing a name

Retention deletes the oldest artifacts, and both obvious ways to decide "oldest" are wrong here:

- **By name, with a suffix only on collision.** `-` (0x2D) sorts before `.` (0x2E), so `<stamp>-1.log` sorts ahead of `<stamp>.log` and the *first* file written sorts last. Pruning would keep the oldest crash and delete the newest.
- **By name, with the first free suffix.** Pruning frees the low numbers it deletes, so the next crash in the same second reuses one. A crash loop then writes nothing but `-000`, each attempt overwriting the name of an artifact just removed.

Both were caught by tests rather than reasoning, which is why the retention rule is spelled out here: order by modification time, which is the fact actually being asked about, and draw suffixes from a counter that only ever increases within a supervisor's life.

### Rejected: tee every line to a file as it is produced

A live per-launch file on the PVC would be greppable during a healthy run, not only after a crash. It was rejected on bounding: a long-running server's output is unbounded, and every way of capping it in place either loses the end (the crash) or needs the same head/tail machinery anyway — plus a write amplification on the PVC for every line of ordinary gameplay chatter. `read_logs` already serves the live case.

### Rejected: per-game declared crash-log paths

Copying each game's own crash dumps (BepInEx's `LogOutput.log`, a JVM `hs_err_pid`) into the artifact would catch loaders that don't tee to stdout. It isn't needed for the games in the catalog — BepInEx's console output does reach stdout, verified on a live pod — and it would reintroduce exactly the per-game adapter the platform exists to avoid. Worth revisiting only for a game that proves it necessary.

## Consequences

- A rollback no longer destroys its own explanation. Every rollback result now tells the agent the output was preserved and points at the shell to search it, so the common path needs no reasoning about where evidence lives.
- Preserved artifacts survive pod recreation, which the in-memory ring never did. The Agones `Unhealthy` path and the watchdog both inherit this for free.
- Memory cost is bounded at ~15000 lines per launch regardless of uptime; disk cost at five artifacts per instance.
- A launch that produces no output writes nothing, so an instance that has never crashed has no `.grizzly/crashes/` directory at all. Its absence is meaningful.
- The middle of an extremely long launch is elided, and says so. A reader who cannot see the gap would infer an ordering that isn't real.
