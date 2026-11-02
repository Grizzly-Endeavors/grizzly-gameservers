# Pre-existing work and work built during the event

The cutoff is 10:30 AM PT on 2026-11-02, the start of the Fireworks Deploy hackathon build window. The split below comes from the git history: `git log --since="2026-11-02 10:30 -0800"` lists the work built during the event, and everything older is pre-existing.

## Table

| Area | Pre-existing (before 10:30 AM PT) | Built during the event |
|---|---|---|
| Gary agent loop | The model-driven loop, escalation exit, per-channel conversations, deferred tasks, proactive watchdog, guardrails (snapshot, apply, verify, auto-rollback) | Per-turn model profile on operator turns, so one request can pick a provider and model; a flag that turns memory off for a turn; capture of reasoning, tools, and full messages for operator turns; one Gary across all of an owner's servers |
| Gary tools | The typed tool surface: file read and write, edit, read-only shell, logs, restart, resource resize, mods, backups | Gary can switch Valheim worlds and Factorio saves per instance; Gary's tools exposed to Claude Code through an MCP server (`evals/harness/gary_mcp.py`) |
| Supervisor and control API | The in-pod supervisor and its typed control API | No change |
| Operator harness | The bearer-gated operator routes (`tool`, `gary`, `tools`) and `just operator` | The evaluation connection helper (`evals/harness/operator_conn.py`) and operator-turn transcript export |
| Game catalog | Game templates and images, including Minecraft, Valheim, and Factorio | Catalog allowlist widened for Valheim and Factorio so the eval tickets are reachable; namespace quota raised for the parallel eval fleet |
| Evaluation | None | Scenario catalog (`evals/scenarios.yaml`) and the evaluation plan (`evals/evals.md`); dev and scored case files and world fixtures; the runner (`run.py`), scorer (`score.py`), split checker (`check_split.py`), and fine-tuning exporter (`export_sft.py`); `any_of` and `tool_regex` check types; the estimated human support row (`human_estimate.json`); smoke results |
| Fireworks | None | Fireworks model wiring: system definitions for GLM 5.3 Flash in Gary and in Claude Code, and the rate table (`models.json`) |
| Web console | None | The web console for server owners and hosting teams, its live mode wired to the operator harness, and its deploy pipeline behind basic auth |
| Fine-tuning | None | Export of dev-split Gary turns as Fireworks fine-tuning JSONL. No tuned model is part of the results unless the results say so |
| Pitch material | None | The pitch deck and script |
| CI | Bot image build and deploy | Faster scoped hooks and a cheaper bot image build; the bot deploys without the gate; web-console image pipeline |

## Material use of AI coding assistants

Claude Code orchestrated agents wrote most of the hackathon code. The team reviewed it and can explain it.

## Evaluation data

- The ticket types come from host knowledge-base article titles and search-result text. No real customer data, credentials, or data from friends' servers is in the repository.
- The case files were generated from the scenario catalog by agents. Dev and scored cases are separate variants, and `evals/harness/check_split.py` checks that they do not overlap.
- The Human T1/T2 support row is an estimate from published IT-helpdesk benchmarks. It is not measured. See `evals/human_estimate.json`.
