# ADR-032 — Per-game guidance loads on demand through a tool, not on every turn

**Status:** Accepted (2026-09-08)

## Context

Mod management is the one part of Gary's job that is genuinely per-game. Minecraft's install is a set of keys in `.grizzly/env` that itzg re-resolves at every start, and removing a mod means *rewriting* the list — never deleting the line, because a missing key makes the image skip reconciliation entirely. A Thunderstore game's install is one pinned package per line in `.grizzly/mods`, and removing a mod means *deleting the line*, because the installed set is rebuilt from that file at every launch. The two rules are exact opposites, stated in the same vocabulary, about files with near-identical names.

Both workflows are long — roughly 800 tokens together — and until now the Minecraft pair was appended to the system prompt of every manager turn, in every guild, whether or not that guild had ever created a Minecraft server. Issue #61 measured the manager prompt at ~6,800 input tokens. Adding Valheim's workflow the same way would have added a second block that everyone reads and most nobody needs, and the third r2modman game after it a third.

The obvious narrowing — include a game's block only when the guild actually runs that game — was rejected on its own terms: the system-prompt builder is handed a comma-joined catalog listing and has no access to the guild's live server set, so it would need plumbing that exists nowhere else; and it still pays the cost on every turn in a guild that *does* run the game, including the overwhelming majority of turns that have nothing to do with mods.

## Decision

**A `game_guide(game)` tool returns the workflow for one game, and the system prompt keeps only a pointer at it.** The pointer is one instruction: before searching for, recommending, installing, or removing a mod on a server, call `game_guide` for that server's game and follow it — including when it says the game takes no mods here.

The guide is composed in Rust from prompt files rather than stored as one blob per game: an intro naming the game, then the registry's "what to install" block, then its "how to install" block. The two Minecraft blocks moved into that structure **verbatim** — this ADR is about *when* the text is read, not what it says.

Three results have no registry blocks at all: a real game that declares no registry gets a final answer that also forbids improvising an install through the file tools, and an unrecognised game gets the catalog listing plus a pointer at `about_server`.

The tool is **manager tier** and targets no server, so it dispatches ahead of the scope gate. It reaches nothing — the catalog is already in memory — which is what makes it cheap enough to be a required first step rather than a lookup Gary weighs.

## Consequences

The manager system prompt drops both game-specific blocks and gains one sentence. Every manager turn that isn't about mods now pays nothing for mod guidance, and the turns that are about mods pay for exactly one game's worth.

Onboarding the third r2modman game costs nothing in prompt size: it reuses the Thunderstore blocks, and its guide is selected by the registry its catalog entry already declares.

The cost is one extra round trip on a mod request — the model calls `game_guide` before `search_mods`. That is the same shape as the `search_mods` → `mod_info` step already required, and it buys the thing the prompt could never guarantee: the model reads the *right* game's rules rather than the ones that happened to be in front of it.

A model that skips the tool and improvises still has the tool descriptions, which name their own game's identifier shape. That is a weaker guard than the guide, and it is the residual risk this decision accepts in exchange for the prompt not carrying every game forever.

## Alternatives considered

**Keep appending every game's workflow.** Rejected on cost that grows with the catalog, and on a subtler failure: two opposite removal rules sitting side by side in one prompt is a worse instruction than either alone, because nothing in the text says which one applies to the server at hand.

**Gate the blocks on the guild's live game set.** Rejected: needs plumbing the system-prompt builder does not have, and still pays on every turn in a guild that runs the game.

**One prompt file per game rather than per registry.** Rejected as premature: the workflow is a property of the registry, not the game, and Valheim's and the next r2modman game's are identical. Composing sections in Rust leaves room for a per-game quirk to slot in as an extra block if one ever turns up.
