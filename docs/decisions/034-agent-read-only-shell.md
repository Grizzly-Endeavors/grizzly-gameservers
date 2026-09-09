# ADR-034 — Gary gets a shell, and the kernel is what makes it read-only

**Status:** Accepted (2026-09-09)

## Context

A friend asked Gary to work out which Valheim mods survived the 1.0 update. The server crashed on startup, the guardrail rolled the change back, and Gary had nothing to say about *why* — which is the whole question. Tracing it turned up three separate ways the evidence dies, and one reason he couldn't have read it even if it had lived.

The evidence dies because the only log source that exists for every game is `LogBuffer`: a 1000-line in-memory ring, created once per supervisor process and shared across every child launch. The supervisor relaunches a crashing child several times before escalating, so by the time the bot observes `Crashed` the ring already holds stacked failed boots; the rollback's healthy restart then floods it. A single healthy Valheim boot measures 3737 lines — one restart overwrites the whole ring more than three times over. If escalation runs all the way to an Agones `Unhealthy`, the pod is recreated and the ring is gone outright.

The game's own crash log is not deleted by the rollback — `fs::restore_file` copies exactly one file back — but for Valheim it is unreachable regardless. BepInEx's live tree is `/opt/valheim/bepinex/BepInEx`, on the container's writable layer; only `BepInEx/config` is symlinked onto the PVC. So `LogOutput.log` sits outside `SUPERVISOR_DATA_DIR=/config`, which `resolve_in_root` refuses; it dies with the pod; and `AppendLog = false` means BepInEx truncates it at every boot. Minecraft is the opposite case — `/data` is the PVC, and rotated `logs/` and timestamped `crash-reports/` survive there — which is what makes this per-game rather than general.

Underneath all of that sits the reason none of it could be recovered by hand: **Gary had no way to search.** `read_logs` returns a trailing line count and nothing else. `read_file` returns a whole file capped at 256KB, and the truncation is a *head* — `bytes.get(..READ_CAP_BYTES)` — so the end of anything long was unreachable by any tool he had. A BepInEx chainloader exception lands near the *front* of a boot; a mid-session crash lands at the back. No fixed window is right, and Gary had only fixed windows.

## Decision

**Give the agent a shell, and enforce read-only in the kernel rather than in the vocabulary.**

A `run_shell` tool at manager tier passes a command line to a new `POST /shell` control route, which runs it under `sh -c` in the server's data directory and returns capped stdout, stderr, exit code, and whether it was truncated or timed out. Pipelines, globs and redirection all parse normally.

Building a bespoke `search_logs` route was the alternative, and it is the wrong shape: every model already drives a terminal fluently, and no route we write gets close to `grep`. The thing worth engineering is not the vocabulary — it is the guarantee.

**Landlock is that guarantee.** The command runs under a ruleset that grants read and execute across the filesystem, write access only to `/tmp` and `/dev/null`, and no file reads at all under `/proc` or `/sys`. Two invariants depend on it:

- **Every config change still takes a snapshot.** The bot arms its auto-rollback only for writes that reported one (`if result.backed_up { ctx.note_pending_change(..) }`). A `sed -i` around that path would silently leave the next crash with nothing to restore — the exact failure this whole investigation started from. Under the sandbox that edit is refused by the kernel, and mutation keeps flowing through the typed tools.
- **Credentials stay out of the channel.** The supervisor's environment holds `SUPERVISOR_CONTROL_TOKEN` and the game's RCON password, and `/proc/1/environ` would hand both to anyone who can ask Gary to read a file.

Parsing commands for "mutating" verbs was never a candidate: `sed -i`, a bare `>`, and a script invoked by name all defeat it, and a guarantee that can be defeated by rephrasing is not one.

`read_file` gains a byte `offset` alongside this, so the tool can page past its own cap instead of the file's tail being permanently out of reach.

### Why Landlock and not a mount namespace

The first design was a private mount namespace with the data root remounted read-only. It cannot work here: ADR-023 drops all capabilities but a bootstrap set, `CAP_SYS_ADMIN` is not among them, and `unshare -m` in a live game pod returns `Operation not permitted`. Landlock needs no capability at all — it is designed for unprivileged sandboxing — and requires only `no_new_privs`, which `allowPrivilegeEscalation: false` already sets. Probed in a running pod, the kernel reports Landlock ABI 6.

### Why the sandbox is applied by re-executing the supervisor

The ruleset is built by re-running the supervisor binary with an internal marker argument and restricting *that* process before it becomes `sh`. Applying it in a `pre_exec` closure would mean allocating between `fork` and `exec` in a threaded Tokio runtime, which can deadlock on the allocator lock. The marker only ever narrows privilege, so argv carries no authority: a forged one yields a confined shell, never an unconfined one.

### Fail closed, always

A kernel that cannot fully enforce the ruleset produces no shell at all. A sandbox that silently degraded would be a write path with no snapshot behind it, which is strictly worse than having no shell — the guarantee would be stated in the tool description and false in the pod.

## Consequences

- Gary can now read anything on a server, including the loader logs outside the data root that were previously invisible to him. `grep`, `sed -n`, `find` and `tail` do the work that a bespoke search route would have done worse.
- `ps` and process substitution do not work in this shell, because `/proc` is not readable. That is the deliberate cost of keeping the control token out of a Discord channel.
- Writes are possible in `/tmp` and `/dev/null` only. "Read-only" means the game's state is immutable from here, not that no byte anywhere can be written — `sort` needs somewhere to spill.
- **This ships in the game images, not the bot image.** A running server keeps its supervisor until it next starts, so `run_shell` answers "older supervisor without shell support" there, and `read_file` reports when an offset was ignored. Both say so rather than degrading quietly.
- A shell reads what still exists, which is only half the problem: the rollback's restart overwrites the evidence within seconds of producing it. [ADR-035](035-preserve-a-crashed-launch.md) closes that half by preserving each dead launch's output to the volume, and the two are only useful together — the artifact is a whole launch, which is a thing worth grepping and not worth reading.
