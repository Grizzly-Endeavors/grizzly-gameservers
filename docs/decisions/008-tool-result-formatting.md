# ADR-008 — Tool results are LLM-facing; only Gary's own replies adapt per surface

**Status:** Accepted (2026-07-12)

## Context

Gary's tool-calling loop (`crate::agent::session::run_session`) runs identically on both surfaces — Discord and in-game chat — but a tool's return string is never shown to a user. It's fed back into the model's own message transcript as a tool result; the model reads it, then composes its own reply in its own words, and only that reply is ever posted (`send_chunks` in Discord, `supervisor_announce` in-game).

`list_servers` and `server_status` had accumulated a `GarySurface` enum (`InGame` / `Discord`) and a shared `agent::render` module that branched their result text on it: a terse positional line for in-game (`"survival (minecraft, Ready, 1.2.3.4:7000)"`), a labeled multi-line-friendly one for Discord (`"survival (game: minecraft, state: Ready, address: 1.2.3.4:7000)"`). The module's own doc comment justified this as preventing the two surfaces' copy from "drifting."

That didn't hold up under inspection:

- The in-game surface exposes exactly these two tools and nothing else (`ingame::agent::ingame_tools`) — it's read-only by design (untrusted player chat never gets the mutating set). So there was no second tool for either format to drift *from*; the "shared module" was infrastructure protecting against a divergence that could only ever happen in these two functions.
- Every other tool Gary has — `browse_files`, `read_file`, `read_logs`, `write_file`, `edit_file`, `send_command`, backups, archives, the whole lifecycle set — has exactly one result format each, a private `format_x` function defined directly in `discord::gary::tools` next to its `exec_x`. None of them route through a shared cross-surface module, because none of them need to: they're Discord-only.
- The actual difference between the two branches was three field labels (`game: `, `state: `, `address: `) — a handful of characters, on output already bounded by a hard reply-length cap and a system prompt instructing brevity. Not a real token/latency optimization, just one that sounded plausible.

The user is never the audience for this text, so "does it read well" was never the right question for it. The right question is "does the model parse it correctly and act on it correctly" — and a model doesn't get more legible input from three trimmed labels; if anything it gets *less* legible, since positional fields ask the model to infer meaning from order rather than being told directly.

## Decision

**Tool results have exactly one format, chosen for LLM legibility, used everywhere.** No `GarySurface` enum, no shared rendering module. `list_servers`/`server_status` now format their own results as a private `format_summary`/`format_server_list` pair living directly in each surface's own file (`discord::gary::tools`, `ingame::agent`) — the same place every other tool's formatter already lives. The two copies are intentionally near-identical text; they're not sharing code because nothing else on either surface shares code for this, and duplicating ~15 lines beats standing up a module to prevent a drift that has no second consumer.

**Only user-facing content adapts per surface.** That's Gary's own composed reply — the words he chooses, tone, length — which already differs appropriately per surface via the system prompts (`build_ingame_system_prompt` asks for one or two short plain-text sentences; the Discord prompt doesn't). Formatting a *tool result* per surface was solving a problem that only exists one layer up, where it's already solved.

**Content may still vary by access tier — by omission, not reformatting.** A read-only caller and an admin see different *tool sets* (`available_tools` gates on `AccessLevel`), which means different information is available to reason about at all. That's a legitimate, existing pattern (defense-in-depth tier checks in `dispatch_mutating`) and is unaffected by this decision — it's omission of whole tools, not a second format for the same tool's result.

## Consequences

- `crate::agent::render` is deleted (67 lines, plus its test module). `agent::mod.rs` no longer re-exports `GarySurface`/`cluster_error`/`format_server_list`/`format_summary`/`no_such`.
- `discord::gary::tools` and `ingame::agent` each gained a small private `format_summary`/`format_server_list`/`no_such`/`cluster_error` — the same shape as every other formatter already in those files. `ServerSummary` is now imported directly from `crate::agones` in both.
- If a third surface is ever added, it gets its own private formatter too, by default — not a vote to resurrect a shared module. A shared module becomes worth it only if a *second* tool on a *second* surface needs the exact same text, which isn't the case today.
- No behavior change for users: neither surface's actual chat replies changed, since tool results were never user-visible to begin with.
