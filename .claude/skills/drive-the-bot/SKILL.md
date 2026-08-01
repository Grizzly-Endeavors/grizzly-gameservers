---
name: drive-the-bot
description: Exercise the deployed grizzly-gameservers bot end-to-end without a human in Discord — run Gary's tools directly, ask Gary a question, inspect live server state. Use whenever a bot/Gary/supervisor change needs verifying against the cluster, when you'd otherwise be about to ask the user to type something in Discord or report back what they see, or when reproducing a bug against a live server. Also use for read-only inspection ("what servers are running", "what's in that config file", "what do the logs say").
---

# Driving the deployed bot

You can drive the live bot yourself. **Never ask the user to run a Discord command on your behalf, and never end a turn with "let me know what it says" — run it.**

The bot serves bearer-gated operator routes on its agent endpoint. `just operator` port-forwards to them, resolves the token from the ESO-synced Secret, and reads Discord ids from the gitignored `.env`. Full rationale in `docs/decisions/011-operator-harness.md`.

## The three commands

```
just operator tools [--access read-only|manager|admin]   # what's callable, with JSON schemas
just operator tool <name> ['<json-args>']                # run one tool directly — the default path
just operator gary "<prompt>"                            # a full model turn
```

**Reach for `tool` first.** It calls Gary's tool directly with no model in the loop: deterministic, fast, and free. Use `gary` only when Gary's *reasoning* is the thing under test — his tool choice, his phrasing, whether a prompt change landed. Driving an LLM to perform an action that has one right answer wastes time and tokens and makes the result nondeterministic.

**Run `just operator tools` before guessing an argument name.** It returns each tool's real JSON Schema.

### Flags

| Flag | Effect |
|---|---|
| `--access <tier>` | Act as `read-only`, `manager`, or `admin` (default). |
| `--scope all` | The cross-guild operator view instead of one guild. |
| `--confirm` | Pre-approve a destructive confirmation. Admin-only. See below. |
| `--new-session` | Start Gary fresh instead of continuing the conversation. |
| `--raw` | Emit the raw JSON — use this when you want to parse the result. |
| `--channel/--user/--guild` | Override the `.env` defaults. |

`--access` is the cheap way to verify tier gating: ask a read-only caller to restart something and confirm it's refused, without anyone creating a second Discord account.

## Reading the output

`gary` returns the reply *and* a trace of every tool the model called with its arguments and result. The trace is usually the more informative half — it shows what Gary actually did, not just what he said he did. `rounds` counts model turns; `ESCALATED` means he burned the round budget and gave up.

The reply is also posted to the Discord testing channel, exactly as a real turn would post it, so the render path is genuinely exercised. Watch the pod with `just bot-logs-deployed` when you need the bot's own logs.

## Safety — there are live users

Real friends use this. Server state is not disposable.

- **Run `just operator tool list_servers` before touching anything.** Know what exists and who it belongs to.
- **Create your own throwaway server to test against** (`create_server`) rather than operating on one you didn't create.
- **Never `--confirm` a destructive action on a server you did not create.** `--confirm` skips the human click on `destroy_server`, `archive_server`, and `restore_server`. Every use posts a visible audit line to the channel, and there is no undo on a destroy.
- Restarting or stopping a server ejects whoever is connected. Check occupancy first, or ask the user, before bouncing something in use.
- Read-only tools (`list_servers`, `server_status`, `read_file`, `read_logs`, `browse_files`, `list_backups`, `list_archives`) are always safe to run.

## When it doesn't work

- **`no operator token available`** — the OpenBao secret isn't provisioned, so the routes aren't mounted. The message names the `bao kv put` command to fix it.
- **`503 ... gateway isn't connected yet`** — the bot is still starting after a rollout. The script already retries; if it persists, the Discord token or the gateway is broken, which is a real incident.
- **`unknown tool`** — a typo. Run `just operator tools`.
- **A tool result reads like a refusal** — that's the real tier or guild-scope gate, not a bug. Check `--access` and whether the server belongs to the guild you're scoped to.

## What this can't do

`/config` and `/gary-home` are Discord slash commands with no tool equivalent, so per-guild admin/manager grants and home-channel toggles still need a Discord client. They're one-time-per-guild setup, so this rarely blocks anything — if you need one, ask the user.
