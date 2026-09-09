# r2modman Profile Codes — Design

**Status:** Implemented. The decision is [ADR-033](../../decisions/033-r2modman-profile-codes.md); this document remains the design of record for how profile codes work, and the "What implementation settled" section at the end records the three questions it left open. Extends the shipped mod reconciler in [`design.md`](design.md) (same directory) and [ADR-031](../../decisions/031-supervisor-reconciles-thunderstore-mods.md).

> This document must stand on its own: it will be implemented in fresh sessions that have only this doc and the codebase — not the conversation that produced it. Every external fact it relies on is stated with how it was verified, because the profile format is r2modman's and is documented nowhere.

## Goal & context

r2modman and Thunderstore Mod Manager let a player export their mod setup as a **profile code** — a UUID another person pastes to get the identical set. It is how modded Valheim groups actually agree on a modpack.

Two things follow from that, and the second is the more valuable:

- **Import.** A friend hands Gary a code and the server ends up running its share of that pack, instead of the friend reading mod names out one at a time.
- **Export.** A player asks Gary for a code and pastes it into their own launcher. This is the half that fixes a real recurring pain: modded Valheim breaks when a client's versions drift from the server's, and a code minted from the server's own pins makes everyone match **by construction**.

The shipped reconciler already resolves a list of `Namespace-Name-Version` strings. A profile is a list of the same strings. The fit is close enough that most of this design is about the one place the two genuinely disagree — a *client* profile is not a *server* mod list — and about not losing that difference.

## Verified facts

Verified live on 2026-09-09 against the real API and against r2modman's source at `ebkr/r2modmanPlus@develop`.

### The wire format

`GET https://thunderstore.io/api/experimental/legacyprofile/get/{key}/` answers `302` to a blob. The body is the literal line `#r2modman` followed by base64 of a zip. Verified against a real code (`01a0838e-8316-f62c-48aa-7bdd59d3f8da`), whose zip held:

```
     241  changelog.txt
    5592  config/BepInEx.cfg
    1460  doorstop_config.ini
     589  export.r2x
```

`export.r2x` is YAML:

```yaml
profileName: Default
mods:
  - name: denikson-BepInExPack_Valheim
    version:
      major: 5
      minor: 4
      patch: 2333
    enabled: true
```

**`name` is the `Namespace-Name` pair, and nothing in the payload is a mod file** — names and versions only, with the files fetched from Thunderstore at install time. That is why a profile code does not weaken the "every installed artifact comes from a moderated registry by name" property the catalog is built on. r2modman's own `ExportMod.fromFullString` parses `(.+)-\d+\.\d+\.\d+$` (`src/model/exports/ExportMod.ts`) — the same grammar `crates/supervisor/src/thunderstore/types.rs` already implements for `PackageRelease`.

`enabled: false` entries are legal: r2modman lets a player disable a mod without removing it from the profile.

### Creating a code

`POST https://thunderstore.io/api/experimental/legacyprofile/create/` takes the payload as the raw request body and answers `{"key": "<uuid>"}`. No authentication. Verified end to end by minting `01a08393-e378-353a-e217-914bbb5c05b3` from a hand-built mods-only export and fetching it back byte-identically — so **a zip containing only `export.r2x` is a valid profile**; the config tree is optional.

**There is no delete endpoint.** The API surface is create and get. Every code ever minted is permanent and fetchable by anyone holding the UUID.

The payload size limit is unknown — the sample was 4.5 KB and the minted one 442 bytes. Cap on our side rather than discovering theirs.

### What r2modman does on import

It resolves dependencies itself, and it force-installs packages the community has **pinned** regardless of whether the profile lists them. `denikson-BepInExPack_Valheim` carries `is_pinned: true` on its package record and the import screen labels it "Pinned". Verified by importing a generated code into a real r2modman install: all four listed packages appeared, the loader pack among them, flagged as pinned.

**Consequence:** an export need not include the loader pack for the import to work — the client side does its own resolution exactly as the supervisor does. This is a coupling to r2modman's behaviour rather than to the format, so it is written down here rather than left as folklore.

### What the config tree actually contains

The `config/` directory in a real export holds **loader-level** configuration (`config/BepInEx.cfg`), and the zip root holds `doorstop_config.ini` — the loader's launcher config. Neither is a per-mod setting; both are owned by the game image and managed by lloesche's updater. This is the opposite of the natural assumption and is the reason the config tree is out of scope below.

### A worked classification

The verified sample profile, run through the shipped `mod_info`, is a useful fixture because it breaks in every interesting place:

| Profile entry | Registry side | Server installs | Player needs |
|---|---|---|---|
| `denikson-BepInExPack_Valheim-5.4.2333` | — (loader pack) | no — image provides it | yes (arrives via pinned/dependency) |
| `ValheimModding-Jotunn-2.29.2` | not stated | yes | yes |
| `RandyKnapp-EquipmentAndQuickSlots-3.0.2` | client only | **no** | yes |
| `Advize-PlantEverything-1.20.0` | server and every player | yes | yes |
| `OdinPlus-OdinHorse-1.6.6` | not stated | yes | yes |

Note that "not stated" is two of five. It is the normal case, not an edge — libraries in particular are almost never tagged.

## Decisions (made by the user, binding)

1. **`.grizzly/mods` becomes the group's modpack, not just this server's install list.** What the server installs is a derived subset. This is a change in what the file *means* and is the decision the rest of the design hangs off.
2. **A client-only entry is marked with a `client:` prefix** — `client:RandyKnapp-EquipmentAndQuickSlots-3.0.2`. One file, one diff, greppable; precedent is Minecraft's `datapack:slug` in `MODRINTH_PROJECTS`.
3. **A bad client-only entry must never stop the server.** A mod the server does not install has no business taking the world down, so the reconciler records `client:` lines without resolving them and is *lenient* about them — the exact opposite of its posture on install lines. Validation moves to the bot, at write time, where `mod_info` already runs.
4. **An exported code is cached and re-minted only when the set changes** — a pinned version moving, or an entry added or removed. Repeated asks return the same code.

## Shape

### 1. The mods file

Grammar gains one form. An install line is unchanged (`Namespace-Name` or `Namespace-Name-1.2.3`); a client line is the same preceded by `client:`.

```
# the group's modpack — the server installs the unprefixed lines
ValheimModding-Jotunn-2.29.2
Advize-PlantEverything-1.20.0
client:RandyKnapp-EquipmentAndQuickSlots-3.0.2
```

**Client lines must carry a version.** The reconciler never resolves them, so nothing downstream can supply one, and an export without a version is useless to a launcher. The bot always has the version at write time (from `mod_info` or from the imported profile), so this costs nothing in practice. A client line that is unpinned or malformed is **skipped with a warning in the log buffer, not a launch failure** — per decision 3. The duplicate rule still applies across both forms: a package may appear once, as one or the other.

### 2. Supervisor

Parsing gains the prefix; resolution, download, staging and the live mirror are untouched, because a client line never enters the resolution set. `resolved` gains the client entries, marked, so it becomes the complete picture of the modpack and its install status — which is what makes it the single artifact the export is built from:

```
Advize-PlantEverything-1.20.0
ValheimModding-Jotunn-2.29.2
# client-only, not installed here: RandyKnapp-EquipmentAndQuickSlots-3.0.2
# denikson-BepInExPack_Valheim is supplied by the game image and was not installed
```

The staged tree and its skeleton are unaffected: a client entry stages nothing.

### 3. Bot — import

`import_profile(server, code)` fetches the blob, decodes it, parses `export.r2x`, and rewrites `.grizzly/mods` through the ordinary file path — so the change is snapshotted, the restart verifies it, and a bad result rolls back, all with no new code.

Per-entry rules:

- `enabled: false` → dropped. It is a mod the exporting player chose not to run.
- A package named in the game's `SUPERVISOR_THUNDERSTORE_PROVIDED` → dropped. The image owns it.
- Registry side `client only` → written as a `client:` line.
- **Everything else, including untagged, is written as an install line.** The asymmetry is deliberate: installing something the server did not need costs a download, while failing to install something it did need leaves the pack broken in a way nobody can see. With "not stated" being the common case, that is the right way round.

Bounded: reject a payload over 1 MiB or a profile over the resolver's existing 256-package cap, before decoding rather than after. Profile text is third-party input and enters Gary's context under the same posture as registry results and game logs (issue #81 applies).

### 4. Bot — export

`export_profile(server)` reads `resolved`, builds an `export.r2x` from the installed entries **plus** the `client:` entries, zips it with no config tree, prefixes `#r2modman`, posts it, and hands back the code.

The loader pack is deliberately **omitted**: r2modman force-installs it as a pinned package, and the mods that need it declare it as a dependency, so it arrives either way. Including it would mean inventing a version the bot does not know — the image's copy is whatever lloesche installed, and `resolved` records only its name.

**Caching.** A code is stored beside `resolved` (`.grizzly/thunderstore/profile`) as a fingerprint of the exported set plus the code it produced. A request whose fingerprint matches returns the cached code; anything else mints a new one. On the PVC rather than in Postgres so it needs no database, rides the whole-volume archive with everything else in `.grizzly/`, and sits next to the file it is derived from.

Two properties fall out of caching and both matter: N friends asking produces one mint rather than N permanent artifacts on somebody else's service, and a code handed out yesterday still describes what the server ran yesterday. The second is also the cost — a saved code goes stale silently when the pack changes, and there is no way to revoke it. Gary should say what a code contains when he hands it over rather than presenting it as a standing address.

### 5. Tiers and guidance

`import_profile` writes the mod list, so it is **manager** tier like every other file edit. `export_profile` changes nothing on the server and is the thing players are meant to ask for, so it is **read-only** tier — which is only safe *because* of the cache: without it, read-only callers could mint unbounded permanent artifacts on a third party's service.

The Thunderstore blocks in `game_guide` gain the two workflows: that a code is the fast path both directions, that an imported profile is the group's pack and the server installs its share, and that a code handed to a player is a snapshot rather than a subscription.

## Integration with existing system

- **Snapshot → apply → verify → auto-rollback.** Untouched. Import is a write to one text file through the existing tools; export writes nothing to the server.
- **Egress.** None needed: `thunderstore.io` is public and the namespace policy already permits the public internet (only RFC1918 destinations need carve-outs). No new credential — the create endpoint takes no authentication.
- **Provenance.** A profile carries no files, so the "every artifact comes from a moderated registry by name" property survives intact. This is worth re-checking if the format ever gains embedded content.
- **Config tiers.** No new tier. `.grizzly/mods` changes meaning, not location.

## What implementation settled

Three things the design left open, and two it did not anticipate.

**An untagged mod installs**, as designed. Nothing observed so far argues the other way; if groups turn out to import packs whose untagged entries are mostly cosmetic, the cost of the current choice is wasted downloads and a longer `resolved`, and reversing it is one arm of `plan_import`.

**A stale `resolved` is the answer, not a problem to route around.** `export_profile` reads it as designed and the tool result says what it describes — the last launch. A server stopped mid-edit exports what it last actually ran, which is the honest answer to "what is this server running"; reading `.grizzly/mods` instead would lose the resolved version of every unpinned entry, and a profile without versions is useless to the launcher on the other end. A server that has never launched with mods has no `resolved` at all, which is reported as a timing answer with an obvious next step rather than as a failure.

**Thunderstore's payload limit is still unprobed.** Ours stands at 1 MiB, checked before the base64 is expanded, alongside the 256-package cap the resolver already enforces. Both are refusals with named reasons, so hitting either says which.

**How the bot learns what the image supplies.** `SUPERVISOR_THUNDERSTORE_PROVIDED` is a Dockerfile `ENV` — baked into the image, absent from the pod, and therefore invisible to the bot, which is the half that needs it at write time. The list is restated as a `thunderstore-provided` annotation on the game's `GameServer` template, matching how `thunderstore-community` already lives in both places, with a drift guard in the bot's `tests/config.rs` that reads the Dockerfile and the manifest and fails CI when they disagree.

**Two cases the per-entry rules did not cover.** A package this community has no record of is **left out and named** rather than written and discovered — writing it stops the server on the next launch and gets rolled back, spending a restart to say what the lookup already said. And when *every* entry is unknown, the import is refused outright rather than writing an empty list, because a profile exported for a different game looks exactly like that and would otherwise uninstall everything the server runs. Separately, a registry that cannot be *reached* refuses the whole import: retrying is free, while a list written from unclassified entries is durable, silently wrong, and looks like it worked.

**One contract moved.** `resolved` stopped being a supervisor-local convention. Its shape, the working directory it sits in, and the name of the cached-code file all live in `grizzly-control-api` beside the route constants, because the bot now parses the file the supervisor writes. An image that overrides `SUPERVISOR_THUNDERSTORE_DIR` still works in-pod but hides `resolved` from the bot, so the supervisor warns at startup when one does.
