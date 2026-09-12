# ADR-039 — A removed mod's settings are swept once the launch has proven good

**Status:** Accepted (2026-09-12)

## Context

[ADR-031](031-supervisor-reconciles-thunderstore-mods.md) made the mod list the whole truth about a server's *files*: the staged tree is rebuilt from scratch every launch and the image's hook mirrors `plugins`/`patchers`/`monomod` with `--delete`, so deleting a line deletes the mod. The same hook mirrors `config/` with `--ignore-existing` and no `--delete`, deliberately — a package ships defaults, and re-copying them every boot would silently revert every value a friend tuned.

The cost of that exception is that nothing ever removes a mod's settings. A live Valheim instance whose mod list named ten packages held seventeen entries in `/config/bepinex`, six of them settings for mods it no longer runs: `jg224.chestflow.cfg`, `org.bepinex.plugins.mining.cfg`, `valheimstorage.movablestackbutton.cfg` and three more. Gary is the only reader of that volume, and this is what he sees when he goes looking for a setting: mostly files describing mods that aren't installed. He greps, finds a plausible one, edits it, and the edit lands on nothing.

Two facts constrain any fix.

**The residue is written by the loader, not shipped by the package.** The staged `config/` tree on that instance was *empty* — every one of those files was created at runtime by BepInEx's `Config.Bind`. So the obvious design, recording what each package installed and removing it when the package leaves, would have caught none of them.

**The filename is a plugin GUID, and a plugin GUID is not derivable from a package name.** `Smoothbrain-Mining` writes `org.bepinex.plugins.mining.cfg`; `mtnewton-ItemStacks` writes `net.mtnewton.itemstacks.cfg`. There is no naming rule to exploit, and no mapping anywhere in the platform.

## Decision

**Attribute each settings file by reading the two artifacts, and sweep after the game reports ready.**

BepInEx stamps `## Plugin GUID: <guid>` into the header of every settings file it generates, and that GUID is a `BepInPlugin` attribute argument — so the same literal string is present in the assembly that declares it. **A settings file is orphaned when its GUID appears in none of the staged assemblies.** Verified against the live instance before building it: all ten installed packages' settings matched their own package's DLL exactly (`org.bepinex.plugins.backpacks` → `Smoothbrain-Backpacks/Backpacks.dll`), and both tested orphans matched nothing.

Four properties of that rule are why it was chosen over the alternatives:

**It needs no memory between launches.** The question is answered from the settings directory and the staged tree as they are right now. Nothing has to be recorded, so nothing can be recorded wrongly, and a pod recreated onto an old volume reaches the same answer as one that has been running for a month.

**Every ambiguity keeps the file.** A file with no GUID header is not a candidate at all — which is how BepInEx's own `BepInEx.cfg` is protected without being named. A GUID claimed by any staged assembly is live even when the match is incidental (ExtraSlots embeds ConditionalConfigSync's GUID as a soft dependency; keeping that file is the right way to be wrong). A `.cfg.grizzly.bak` snapshot is not a candidate, because `.cfg` is matched exactly and those belong to the rollback path. The sweep can fail to remove something, which costs one stale file; it must not remove something live, which costs a friend's settings.

**It lives in the supervisor, not in the image's hook.** lloesche symlinks `BepInEx/config` to `/config/bepinex`, so the live settings directory is on the PVC and inside the data root — the supervisor can reach it. The image declares where it is (`SUPERVISOR_THUNDERSTORE_CONFIG_DIR`) and which entries it owns (`SUPERVISOR_THUNDERSTORE_CONFIG_KEEP`, for the baked RCON plugin whose assembly is not in the staged tree), which is the same three-environment-variables onboarding shape ADR-031 established. Header parsing, directory attribution and the decision are pure and unit-tested; the alternative was more `grep` in a game image, which no test reaches.

**It runs on every ready, not before the launch.** The launch that needs a sweep is the one that just applied a mod-list change — a restart. Reaching ready is what proves that launch survived, and a mod list that stops the server is restored and restarted by the bot. A sweep that ran before the launch would have already deleted the settings of the mod the rollback just put back. It runs in `spawn_blocking`, since it reads every staged assembly and nothing waits on the result: the game is already serving players.

**Orphans are deleted, not quarantined.** The mod list is the whole truth, and a quarantine directory is a second place for the agent to trip over. The cost is that re-adding a mod starts from default settings; the post-ready timing is what keeps that cost off the rollback path, which was the only case where it would have been lost without anyone choosing it.

A directory in the settings tree is judged by its name (a plugin that keeps several files names its directory after its GUID) and removed as a unit, and only when its name contains a dot — so a directory that plainly isn't a GUID, like the stray `plugins/` on that same instance, stays out of the sweep entirely rather than surviving on an incidental byte match.

## Consequences

The settings directory now reads as the current install set, which is the property that makes it usable: a settings file Gary can find belongs to a mod that is running. The per-game guidance says so, because without it he has no reason to trust what he finds and the habit that follows — read a stale file, edit it — produces a change that appears to land and does nothing.

Each removal is named in the `[mods]` log lines and in the supervisor's own tracing. The log line is a convenience rather than the record: the sweep fires at ready, and on a chatty game the flood of startup output that follows can push it out of the bounded child-output buffer within seconds — observed on Valheim. The durable answer to "where did that settings file go" is `resolved`, which says the mod isn't installed, and the invariant the per-game guidance states: a settings file that is still there belongs to a mod that is still installed.

A game whose image sets no settings directory sweeps nothing and behaves exactly as before. The sweep is off for every game but Valheim today.

The attribution rule is a heuristic about a third party's file format, and it can be wrong in one direction: a plugin that builds its GUID at runtime rather than declaring it as a literal would have its settings read as orphaned. Nothing observed does this, and the blast radius is one file of settings that the plugin regenerates at defaults on its next load — but it is the failure mode to look for if a friend reports a mod's settings resetting.

## Alternatives considered

**Record what each package installed and remove it when the package leaves.** Rejected on evidence: the staged `config/` tree was empty on the instance that has the problem. Every orphan was written by the loader at runtime, so a manifest of shipped files would have caught none of them.

**Mirror `config/` with `--delete` like the other three directories.** Rejected — it deletes every tuned value on every restart, which is the exact thing `--ignore-existing` exists to prevent, and it deletes the runtime-written files too since they are in no package.

**Match the plugin name in the header against the DLL filename.** Rejected as strictly weaker than the GUID match and wrong in observed cases: `Movable ValheimStorage Stack Button` names no file, and the GUID literal is present in the assembly whether or not the names line up.

**Read the loaded-plugin set out of BepInEx's log.** Rejected on timing. The log names what the *previous* launch loaded, so a mod removed now would have its settings swept a restart later — and it puts per-game log parsing in the supervisor, which ADR-031 exists to avoid.

**Quarantine orphans and restore them if the mod is re-added.** Considered seriously, and it is the symmetric rule — the same GUID match that evicts a file brings it back. Rejected in favour of deletion: it grows a second directory of derived state forever, and once the sweep waits for a proven launch, the case it protected against (a rolled-back mod list) can no longer happen.
