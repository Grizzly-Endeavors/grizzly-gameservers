# ADR-023: Game pods are confined but not rootless

## Status

Accepted.

## Context

Game pods executed mods, plugins and datapacks chosen by friends while running with no `securityContext` at all — verified live on a running Palworld instance, where the container's `securityContext` was absent and the pod's was `{}`. That means uid 0, the container runtime's full default capability set, a writable root filesystem, and — the part that is easy to miss — **seccomp unconfined**. Kubernetes applies no seccomp profile unless a manifest names one; an absent `securityContext` is not "the Docker default", it is no filter at all.

The stated reason for leaving all of it unset was real: the itzg / factoriotools / thijsvanloef / wolveix / lloesche / ryshe bases all bootstrap as root. They chown the mounted data dir to a configurable PUID/PGID and then drop privileges with gosu or su-exec before launching the game. A forced non-root user or a read-only root filesystem breaks that bootstrap.

The error was treating the hardening baseline as one indivisible thing. That reasoning is sound for exactly two controls — `runAsNonRoot` and `readOnlyRootFilesystem` — and load-bearing for neither of the others. Because the baseline was deferred as a block, and the block was filed behind game-image CI signing and an Agones Kyverno carve-out (issue #3, both expensive), the cheap and valuable part stayed undone while the expensive part waited.

This matters more now than it did: the guild allowlist and per-guild caps exist because the platform is opening to people it does not vet, and a mod is arbitrary code from a registry, running in this pod.

## Decision

Game pods apply every hardening control that does not conflict with a root bootstrap, and leave unset only the two that do.

Applied on every game, including `games/_template/`:

- **`seccompProfile: RuntimeDefault`** at the pod level — the runtime's own filter, which every one of these images already runs under in plain Docker. One line, and the main defence against a kernel-exploit escape.
- **`allowPrivilegeEscalation: false`** — `no_new_privs` blocks *gaining* privilege. gosu and su-exec only shed it, so the bootstrap is unaffected.
- **`capabilities: drop: [ALL]`**, adding back `CHOWN`, `DAC_OVERRIDE`, `FOWNER` (chown/chmod the data dir), `SETUID`, `SETGID` (become the game user), and `KILL` (root stripped of this capability cannot signal a child running as another uid, which the supervisor does as PID 1). Everything else in Docker's default set is gone, `NET_BIND_SERVICE` included — no game in the catalog listens below port 1024.

Unset, with a documented `nosemgrep` line each: `runAsNonRoot` and `readOnlyRootFilesystem`.

The capability list is one set shared by all six games rather than a per-game minimum. A per-game list would be tighter, but it would need six separate empirical narrowings to establish and would drift silently as bases change; the shared set is the union of what a root-chown-then-drop bootstrap needs, and a game that needs fewer may add back fewer.

Three drift guards in `crates/bot/src/tests/config.rs` read the real manifests off disk and fail CI if a game omits seccomp, omits `allowPrivilegeEscalation: false`, fails to drop `ALL`, or adds back a capability outside the bootstrap set. They cover `games/_template/` alongside the six real games, because an omission there propagates into every game later copied from it.

## Consequences

- The pod still runs as uid 0 with a writable root filesystem. This closes the containment gap, not the root gap; a mod that escapes the game process still lands as root inside the container. Ending that requires either bases that run rootless or a rebuild that inverts the bootstrap, and neither is in scope here.
- The change ships in the **bot** image, not the game images: the renderer builds each instance from the `games/` catalog baked into the bot, and passes the pod spec through verbatim. No `just game-push` is needed, and the hardening applies to every game at once.
- It takes effect on the next render, so a newly created or cold-started server is hardened while an already-running pod keeps its current spec until it next starts. Existing servers are not restarted to pick it up.
- The remaining two controls, game-image CI signing, and the Agones sidecar Kyverno exception stay in issue #3. Splitting the pod hardening out is what let it land without them.
- A new game copied from the template is hardened by default and cannot quietly regress, which is the part that survives the people who wrote it.
