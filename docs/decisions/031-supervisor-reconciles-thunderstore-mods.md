# ADR-031 — The supervisor reconciles a declarative mod list before each launch

**Status:** Accepted (2026-09-08)

## Context

Minecraft mod management installs nothing itself. itzg's entrypoint re-resolves `MODRINTH_PROJECTS` at every container start, so the platform only had to provide a per-instance place to put that variable ([ADR-012](012-per-instance-env-overlay.md)). That is a property of one upstream image, not a property of game modding.

The r2modman/Thunderstore family — Valheim first, and Palworld, Lethal Company and Risk of Rain 2 behind it — has no equivalent. lloesche's `valheim-server` fetches and installs the BepInEx *loader pack*, but the plugins themselves are the operator's problem: its `merge_mod` rsyncs `/config/bepinex/plugins/` into the staged install only at merge time (a pack or game update), without `--delete`. An in-place restart never syncs it, and a plugin removed from the PVC is never removed from the install. Nothing in the image resolves a dependency, and nothing removes anything.

So somewhere in the platform has to own package resolution, download, layout and removal for this whole family. The question is where.

## Decision

**In the supervisor, in the same per-launch preparation chain as the env overlay, driven by one declarative file on the instance's volume.**

`.grizzly/mods` holds one package per line — `Namespace-Name`, or `Namespace-Name-1.2.3` to pin. Before every child launch the supervisor parses it, resolves it transitively, downloads and validates the zips, rebuilds a staged install tree from scratch, and swaps that tree into place.

Four things fall out of that placement, and they are the reason for it:

**The install inherits the existing safety loop unchanged.** Gary edits one text file through the ordinary file tools. That edit is snapshotted like any other, the restart that applies it is the same self-guarding restart, and a mod list that stops the server is restored and restarted automatically. This design adds no rollback code — it reuses all of it.

**No new write surface.** The alternative was the bot downloading packages and pushing them into the pod through a new binary-write control route. [ADR-012](012-per-instance-env-overlay.md) already rejected that shape for Minecraft, and both of its reasons hold harder here: it puts dependency resolution inside an LLM turn, and it puts the installed state *outside* the snapshot-and-rollback loop, where nothing would put it back.

**A failure is a crash.** A malformed line, a package that doesn't exist, a download with nothing cached, a zip that fails validation — each fails the launch, and the supervisor reports the phase as `Crashed` without touching the restart counter. `/start` and `/restart` still answer "acted", because the bot only runs its verify-and-roll-back loop when the supervisor says it acted; answering "failed" would skip the very recovery this depends on. The initial spawn takes the same recoverable path, so a pod recreated onto a volume with a bad mod list keeps a reachable control API instead of crash-looping the container out of anyone's reach. Spawn failures proper stay fatal, because a container that can't run its own entrypoint is not repairable from inside the pod.

**The supervisor stages; the image installs.** The reconciler writes only into `.grizzly/thunderstore/staged/`, in the loader-relative layout r2modman established. Each game's own pre-launch hook mirrors that tree into wherever its loader actually reads from — for Valheim, `valheim-pre-server.sh` rsyncs `plugins`/`patchers`/`monomod` with `--delete` and `config/` with `--ignore-existing`, and never touches `core/`. That split is what keeps the supervisor free of any per-game path knowledge: onboarding the next r2modman game is three environment variables and a mirror step in its image, not a branch in Rust.

The tree is **rebuilt from scratch and swapped**, not reconciled in place. Rebuilding is what makes deleting a line delete the files without tracking what each package installed; swapping is what stops a launch that dies mid-extraction leaving a tree that is neither the old set nor the new one, since the image mirrors whatever it finds.

**Resolution rules.** An explicit pin is hard — Thunderstore manifests declare what a package was *built against*, not what it requires, so a dependency asking for more is a preference that warns rather than a constraint that wins. A dependency's version is a floor, and the highest floor wins. Packages the image supplies are skipped whole by `Namespace-Name`. Resolution is cycle-safe (a package is only re-resolved when the version strictly increases) and bounded at 256 packages and depth 32.

**Offline behaviour is a first-class case, not a fallback.** Every cached zip carries the same dependency strings the API reports, so a fully pinned mod list resolves and installs from cache with no network at all. That is what makes a pod recreated during a Thunderstore outage come back with the mods it had. An unpinned entry falls back to the highest cached version with a warning; nothing cached and no network is a failure.

## Consequences

Valheim takes mods through the same conversation shape as Minecraft, with the opposite removal rule — delete the line — which is why per-game guidance became a loaded-on-demand tool ([ADR-032](032-game-guidance-loads-on-demand.md)) rather than a second block in the system prompt.

An unrepaired reconcile failure raises a watchdog incident, because `Crashed` is a crash signal. That is intended: a server that has been down since someone's mod edit is exactly what the watchdog exists to notice.

The per-instance tier grows a working tree (`.grizzly/thunderstore/`: `cache/`, `staged/`, `resolved`) beside `.grizzly/env`. It rides the whole-volume archive, so a recovered server resolves offline from its own cache.

Package names and descriptions enter Gary's context under the same posture as Modrinth results and game logs; issue #81 applies equally.

The reconcile runs inline in the supervise loop under a 180-second deadline. If a first install is ever observed starving control requests, the follow-up is to run it in a spawned task with its own `select!` arm — the deadline and the failure path do not change.

## Alternatives considered

**The bot downloads and pushes packages through a new binary-write route.** Rejected for the two reasons [ADR-012](012-per-instance-env-overlay.md) already gave, both sharper here: dependency resolution inside an LLM loop, and installed state outside the rollback loop.

**A shell hook in each game image does the fetch.** Rejected as untestable — no unit test reaches a `curl | unzip` pipeline in a game image — and as duplication: every r2modman game would copy-paste the same resolution logic, and the fifth copy would be subtly different from the first.

**A key in `.grizzly/env` instead of a dedicated file.** Rejected on two counts: the supervisor should not read its own instructions out of the child's environment, and the overlay's hard-deny on `SUPERVISOR_`-prefixed keys makes the naming awkward. A dedicated file also gets its own grammar and its own failure mode — the mod list fails loudly where the env overlay degrades quietly, which is correct for each.

**Degrade to "install what worked" on a bad entry.** Rejected. It leaves a server running a set nobody asked for, with the file claiming otherwise and nothing failing to notice. Failing the launch routes the mistake into a recovery that already exists.
