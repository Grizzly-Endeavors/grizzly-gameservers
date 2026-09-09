# 036: Context Is Sized Against the Window, and Files Are Addressed in Lines

**Date:** 2026-09-09
**Status:** accepted

## Context

Gary lost track of which server a conversation was about mid-task — asked to change a Valheim mod config, he began editing Palworld's ([#115](https://github.com/BearFlinn/grizzly-gameservers/issues/115)). It was deterministic, not a lapse. The transcript store capped a conversation at 30 messages and 128 KiB (~32k tokens), and its backward walk *stopped* at the first message that didn't fit, discarding that message and every older one. A single 226,232-byte Valheim item config — read whole, because `read_file` capped at 256 KiB and paged by byte offset — was 1.7× the entire budget on its own, so one read took every user turn that had named the server with it.

Three things were wrong underneath that. The budget was sized for a constraint that no longer exists: the model in use has a 1M-token window and cached input costs $0.007/1M, so re-sending a transcript is nearly free while *cutting* one destroys what was said. The 30-message cap was worse than the byte cap — sixteen tool rounds produce at least 32 messages, so any turn using its full round budget had its own opening rounds trimmed at commit. And the byte budget was estimated from message lengths while the real prompt included ~6k tokens of tool schemas it never counted.

## Decision

**Trim on measured tokens, between two bounds.** `TokenUsage.prompt` — the provider's own count for the exact request sent — triggers a trim at 600k tokens, which cuts back to 400k. Both are settings (`gary.transcript_trigger_tokens`, `gary.transcript_target_tokens`). The message cap is gone; tokens are the only bound.

**Address files in lines.** `read_file` takes `start_line` and `line_count` (default 400 lines), returns numbered lines, and reports the file's total line count. Byte offsets are gone from the contract entirely. `edit_file` moves into the supervisor as `POST /fs/edit`: the find-and-replace happens in the pod, so the file never crosses the wire.

## Alternatives Considered

- **Raise the byte budget and elide oversized tool results when persisting** (the issue's own proposal) — Rejected. Rewriting or eliding earlier messages invalidates the cached prompt prefix, which is exactly what makes the re-send affordable; and eliding at commit still leaves the payload riding the transcript for the rest of the turn that read it, which is where the token cost actually lands.
- **Track the conversation's subject server explicitly and render it into the system prompt** — Rejected for the same reason: mutating the system message on every checkout bursts the cache prefix on every turn. It also binds a session to one server, when operating across several is part of what Gary is for.
- **Keep a message-count cap as a runaway guard** — Rejected. It is a second thing that can silently gut context, and the token bound already covers memory, since messages are what tokens are made of.
- **Keep byte offsets for paging** — Rejected. A model cannot derive a byte offset from a `grep` hit or from a window it has already read, only carry forward the one a reply hands it; pages split lines mid-entry; and nothing anchors a later reference to a location.
- **Keep `edit_file` client-side and just page the read** — Rejected. It re-read the whole file with no offset and refused outright on a truncated read (`TooLargeToEdit`), so paging never composed with editing: a file too big to read in one go stayed uneditable no matter how carefully it was read.

## Consequences

- A conversation now survives roughly 20× more traffic before anything is dropped, and a trim is rare enough that the one cache miss it causes is not worth optimising. The hysteresis is load-bearing: trimming to just under the trigger would re-fire every turn and burst the prefix every turn, which is worse than the old behaviour.
- The trigger is measured but the cut is estimated. Usage is reported per request, not per message, so there is nothing to subtract message by message; the cut scales bytes by the bytes-per-token ratio the conversation just demonstrated, then the next turn re-measures. A provider reporting no usage yields zero, read as "no reason to trim".
- `read_file` returning 400 lines instead of 256 KiB is only safe *because* editing moved in-pod. Shrinking reads would otherwise have shrunk what could be edited.
- The two conflated jobs of `READ_CAP_BYTES` are now split: a model-facing window bounded in lines, and a machine-facing whole-file read for callers that parse (`profiles.rs` reading `.grizzly/mods`). A caller that names no window gets the whole file, so a parser cannot silently inherit the model's window.
- **This ships in the game images, not the bot image**, and there is no compatibility path. A bot on the new contract talking to a supervisor predating it gets a whole-file read where it asked for a window, and a 404 where it asked to edit. The three running servers are being restarted to pick up the new image; that is the migration.
- ADR-034's byte `offset` on `read_file` is superseded by the line window here.
