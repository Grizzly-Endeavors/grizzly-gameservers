# ADR-033 — A profile code is the unit a modpack moves in

**Status:** Accepted (2026-09-08)

## Context

[ADR-031](031-supervisor-reconciles-thunderstore-mods.md) gave the Thunderstore family a declarative mod list the supervisor reconciles before each launch. It works one mod at a time, which is the right grain for "install a better map" and the wrong grain for the thing modded groups actually do: agree on a *pack*.

r2modman and Thunderstore Mod Manager already have a unit for that. A player exports their setup as a **profile code** — a UUID somebody else pastes to get the identical set — and that is how a modded Valheim group agrees what everyone is running. The format is r2modman's, documented nowhere, and was pinned down by fetching a real code: the payload is the literal line `#r2modman` followed by base64 of a zip, and the only member that matters is `export.r2x`, a YAML list of `Namespace-Name` packages with their exact versions. **Nothing in a payload is a mod file** — the files are fetched from Thunderstore by name at install time.

Two directions follow, and the second is the more valuable. **In**, a friend hands over a code instead of reading twenty mod names out one at a time. **Out**, a player asks for a code and pastes it into their own launcher — which fixes a real recurring failure, because modded Valheim breaks when a client's versions drift from the server's, and a code minted from the server's own pins makes everyone match by construction.

The obstacle is that the two artifacts are not the same thing. A *client* profile lists everything that player runs, including mods the server has no business installing; a server's mod list has, until now, been only what the server installs.

## Decision

**`.grizzly/mods` becomes the group's modpack, not this server's install list.** What the server installs is a derived subset. An entry prefixed `client:` — `client:RandyKnapp-EquipmentAndQuickSlots-3.0.2` — names a package that belongs on the players' machines: the supervisor never resolves it, never installs it, and carries it through to `resolved` marked as such. One file, one diff, greppable; the precedent is Minecraft's `datapack:` prefix in `MODRINTH_PROJECTS`.

**A bad client entry must never stop the server.** The reconciler's posture on an install line is deliberately loud — a typo fails the launch and is rolled back ([ADR-031](031-supervisor-reconciles-thunderstore-mods.md)) — and the mirror-image reasoning applies here: a mod this server does not install has no business taking the world down. An unpinned or malformed `client:` line is skipped with a warning in the log buffer. The validation that would have caught it moves to the bot, at write time, where the registry is already being consulted.

**Import classifies before it writes.** `import_profile` fetches the payload, decodes it, and asks the registry where each entry has to be installed. A mod the exporting player had switched off is dropped; a package the game image supplies is dropped; a client-only package becomes a `client:` line; **everything else, including untagged, becomes an install line.** That last asymmetry is deliberate: installing something the server did not need costs a download, while failing to install something it did need leaves the pack broken in a way nobody can see — and "the registry didn't say" is the common case, since libraries are almost never tagged. The write goes through the ordinary file path, so the change is snapshotted, the restart verifies it, and a bad result rolls back with no new code.

Two cases the format itself does not settle:

- **A package this community has no record of is left out and named**, rather than written and discovered. Writing it stops the server on its next launch and gets rolled back — spending a restart to say what the lookup already said. Leaving the whole list empty is refused outright, because a profile exported for a different game looks exactly like that and would otherwise uninstall everything the server runs.
- **A registry that cannot be reached refuses the whole import.** Import is cheap and idempotent, so retrying costs nothing; a mod list written from unclassified entries is durable, silently wrong, and looks like it worked.

**Export is read-only tier, and the cache is what makes that safe.** `export_profile` builds `export.r2x` from `resolved` — the installed set *plus* the `client:` entries, which is why `resolved` had to carry them — and posts it. There is **no delete endpoint**: every code ever minted is permanent and fetchable by anyone holding the UUID. So the code is stored beside `resolved` at `.grizzly/thunderstore/profile` together with the exact set it names, and a request for an unchanged set returns the stored code. Without that, any caller who could ask a question could mint unbounded permanent artifacts on somebody else's service, and read-only tier would be indefensible. With it, N friends asking produces one mint.

The loader pack is **omitted** from an export. r2modman force-installs it as a community-pinned package and the mods that need it declare it as a dependency, so it arrives on the player's machine either way; including it would mean inventing a version nobody here knows, since the image's copy is whatever its updater last pulled.

## Consequences

A code handed out yesterday still describes what the server ran yesterday. That is the point of caching and it is also its cost: a code goes stale silently when the pack changes, and there is no way to revoke it. Both the tool result and the per-game guidance say what a code contains at the moment it is handed over, and say it is a snapshot rather than a standing address.

The bot now needs a fact it could not previously see: which packages the game image supplies. `SUPERVISOR_THUNDERSTORE_PROVIDED` is a Dockerfile `ENV`, baked into the image and never present on the pod, so the same list is restated as a `thunderstore-provided` annotation on the game's `GameServer` template — the arrangement `thunderstore-community` already lives under, with a drift guard in the bot's `tests/config.rs` reading both and failing CI when they disagree.

The `resolved` file stopped being a supervisor-local convention and became a contract: the bot parses it to build an export. Its shape, and the working directory it lives in, moved into `grizzly-control-api` alongside the route constants. An image that overrides `SUPERVISOR_THUNDERSTORE_DIR` still works in-pod but hides `resolved` from the bot, so the supervisor now warns when one does.

Profile text is third-party input and enters Gary's context under the same posture as registry results and game logs; issue #81 applies. Payloads are bounded at 1 MiB before decoding and 256 packages after, the latter matching the resolver's own ceiling so an oversized profile is refused while it is still a string.

No egress change and no new credential: `thunderstore.io` is public, the namespace policy already permits the public internet, and both profile endpoints take no authentication.

## Alternatives considered

**Two files — this server's installs, and the pack.** Rejected: the second file would have no reader but the exporter, and the two would drift the moment anyone edited one. Making the existing file mean "the pack" costs one prefix and keeps a single source of truth.

**Fail the launch on a bad `client:` line, like an install line.** Rejected. The loudness of the install path buys something — a mistake nobody would otherwise see, routed into an existing recovery — and a client entry buys none of it, because the server installs nothing for it. Failing there would mean a typo about somebody else's machine taking a world offline.

**Mint a code per request instead of caching.** Rejected on the tier: without the cache, export could not be read-only, and read-only is exactly where it belongs — the people who need a code are the players, and asking a manager to fetch one for them reintroduces the friction the tool removes.

**Read the mod list directly instead of `resolved` on export.** Rejected because an unpinned entry there has no version, and a profile without versions is useless to the launcher on the other end. The cost is that `resolved` describes the last launch, so a server stopped mid-edit exports what it last ran — which is also the honest answer to "what is this server running".

**Carry the export's `config/` tree.** Rejected: what a real export carries there is *loader-level* configuration (`config/BepInEx.cfg`, `doorstop_config.ini`), owned by the game image and managed by lloesche's updater. Importing it would overwrite the loader; exporting it would hand out this pod's copy of somebody else's file.
