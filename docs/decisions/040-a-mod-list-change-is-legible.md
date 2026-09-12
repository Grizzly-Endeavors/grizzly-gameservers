# ADR-040 — A mod-list change says what it changed, and updating is one call

**Status:** Accepted (2026-09-12)

## Context

Two things about the mod list were legible to the platform and not to the agent driving it.

**An import replaces the list.** `import_profile` writes the whole file from a profile code ([ADR-033](033-r2modman-profile-codes.md)), and its result described what the *pack contains* — installs, client-only entries, and three categories of entry that were dropped. What it never said was what the write took *away*. A friend hands over a code because their group agreed a pack; the mods the server had that this code doesn't name disappear on the next restart, and nobody asking for a modpack thinks to check for that. After the write there is nothing left to compare against.

**Updating meant one registry call per mod.** Nothing walked the list. Asked whether the mods were current, Gary called `mod_info` per package — a twenty-mod list is a twenty-round turn that costs twenty times as much, spends the round budget on bookkeeping, and passes every version through the model as a string it then has to retype into a file. That last part is where a wrong pin comes from, and a wrong pin stops the next launch. Pinning is insisted on for good reasons — a pinned list reinstalls identically and resolves from the instance's own cache during a registry outage — so the update problem is one the platform created for itself.

## Decision

**Report the diff on every import, and add one tool that bumps the whole list.**

### The diff is against the declared list

`import_profile` reads `.grizzly/mods` before writing it and reports four categories against what was there: added, removed, version-changed, and moved between the server and the players' machines. When nothing moved it says so outright, because four empty lists is a complete answer only if it is stated as one.

**Against the declared list, not against `resolved`.** `resolved` also holds every transitively-pulled dependency, and a new declared list names none of them — so diffing the installed set would report a dozen removals for a change that removed nothing, and the model would relay that.

**A side change is its own category, not a removal plus an addition.** A mod that moved to the players' side is still in the pack and is no longer running on the server. "Removed" is a wrong answer to the only question that distinction affects.

A read that genuinely fails stops the import rather than writing without a diff — the write about to follow would fail the same way. An absent list is an ordinary state and diffs as all-new.

### `update_mods` takes a required mode

One tool at manager tier, looking up every pinned entry concurrently and either reporting the moves or rewriting the file.

**The mode is a required enum (`check`/`apply`), not an optional boolean.** "Any updates?" and "update them" are the same sentence in most phrasings, and an omitted flag would resolve that ambiguity by writing — dirtying a config file and leaving a pending change hanging on a restart nobody intended. A required choice with both values described is what makes the model pick deliberately. Both modes are manager tier: a check is a burst of calls against a public registry per invocation, which is the cost a read-only tier exists to keep out.

**A pin only ever rises.** "Latest" is the only claim the registry makes. A list pinned above it — a version yanked after someone pinned it, an author who re-published downward — is left alone rather than quietly rolled back to something older than what the server is running.

**An unpinned line is reported, not pinned.** It already resolves to the latest at every launch. Pinning it would *remove* that behaviour under the name of an update.

**The rewrite is line-by-line.** Comments, blank lines, the order somebody chose, and any line nothing understands are carried through byte for byte. Re-rendering the file from a parsed set would discard all of it, and an update has no business editing a line it can't read. A package the registry has no record of keeps its line too: the list may be older than a rename, and rewriting a line whose package can't be confirmed is how an update breaks a launch.

**Client entries are bumped, on their own line.** The list is the group's modpack. An update that moved the server and left the players' half behind produces a version mismatch nobody can see until everyone fails to connect — but it is also the only part of an update that creates work outside the server, so it is never folded into the same list as the free changes.

**A lookup that can't be reached fails the whole call**, the same posture `import_profile` takes and for the same reason: the call is cheap and idempotent, so retrying costs nothing, whereas silently skipping the packages it couldn't check would report a list as current when it isn't — and nobody asks twice.

Deprecated packages are reported whichever mode ran and whether or not the pin moved: "its author has stopped maintaining this" is the same news at either version, and no other turn will mention it.

### Both halves needed the grammar shared

The bot now reads and rewrites `.grizzly/mods`, so its line grammar has two authors rather than one. The package identifiers and the line classifier moved from the supervisor into `grizzly-control-api`, the crate that exists to be the contract between the halves, with each half's own posture layered on top — the supervisor still turns a bad install line into a launch failure naming the line number. One grammar with two authors in two crates is a copy that drifts, and the drift shows up as the bot writing a file the supervisor refuses at launch.

## Consequences

The three tools that read or rewrite a whole mod list — import, export, update — now share `crates/bot/src/discord/gary/mod_list.rs`, which is where the guard confirming the server's game speaks Thunderstore and the read of one file off its volume actually belong.

"Are my mods up to date" costs one round instead of twenty, and the version that lands in the file is never one the model retyped. The per-game guidance names the tool immediately after the pinning rule, because insisting on pins is what creates the staleness the tool exists to fix.

An import is now safe to run on a server with a pack already on it, in the sense that matters: the replacement still happens, but it is reported before anyone is surprised by it. Merging remains explicitly out of scope — an import replaces, and saying so accurately is the fix, not changing what it does.

`update_mods` is the second tool whose whole cost is registry calls against a third party that publishes no rate limit. It bounds concurrency the way the import's classification does.

## Alternatives considered

**Diff against `resolved` instead of the declared list.** Rejected: dependencies appear there and in no declared list, so every import would report removals it isn't making.

**A `check_only` boolean defaulting to false.** Rejected. The default would be the writing one, reached by forgetting a parameter, on the tool whose two intents are hardest to tell apart from a sentence.

**Two tools split by tier — a read-only `check_mod_updates` and a manager `update_mods`.** Rejected: it puts a burst of third-party registry calls at the tier with no write access, and adds a second entry to every manager's tool list for a question that isn't common enough to earn it.

**A `packages` filter to update a named subset.** Rejected as already covered. `mod_info` plus `edit_file` is the single-mod path and works; this tool is explicitly the bulk one, and a filter would give the model a way to half-do the thing it was asked to do.

**Leave the client entries alone and update only the server.** Rejected. It reads as the safer, narrower change and isn't: it silently desyncs the exported profile code from what the server runs, so the next export hands players versions that can't connect.
