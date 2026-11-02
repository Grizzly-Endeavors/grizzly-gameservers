# Evals

The benchmark behind the Fireworks Deploy hackathon submission: game-server support tickets worked by a frontier model in Claude Code, by an open model on Fireworks in Gary's harness, and by that same open model in Claude Code as an ablation. The method, the integrity rules, and the statistics are in [`evals.md`](evals.md). This file covers what is where and how to rerun it.

## Layout

| Path | What it is |
|---|---|
| `evals.md` | The evaluation plan: systems, procedure, pass criteria, measurements, statistics, limits. |
| `scenarios.yaml` | The scenario catalog. Each scenario is a ticket type taken from a hosting provider's knowledge base (`source:`), expanded into variants. |
| `cases/dev/`, `cases/scored/` | One YAML file per concrete case: the customer message, the staging steps, and the checks. Schema in [`cases/README.md`](cases/README.md). Prompts, harness, and game knowledge are tuned on `dev` only. |
| `fixtures/` | World and save files that some cases upload to the server during staging. |
| `harness/run.py` | The runner. For each (system, case, repeat): create a fresh server, stage it, send the customer message, grade, restart once, grade again, destroy the server. |
| `harness/score.py` | Turns run records into `eval_results.json` and the Markdown tables: Wilson intervals, McNemar against the baseline, paired-bootstrap cost and latency ratios. |
| `harness/systems.json` | The systems under test: harness, model, and credentials (by env var name, never by value). |
| `harness/models.json` | The one rate table used to price every system from its token counts. |
| `harness/gary_mcp.py` | An MCP server that gives Claude Code exactly Gary's tool surface, through the same operator route and tier gates Gary uses. |
| `harness/export_sft.py` | Exports passing dev-split Gary turns as fine-tuning JSONL. It refuses to export scored-split records. |
| `results/<run>/` | Committed run records (one JSON per system × case × repeat) plus full transcripts under `transcripts/`. |

## Recompute the tables (no cluster needed)

```bash
pip install pyyaml
python3 evals/harness/score.py evals/results/<run> --cases evals/cases/scored -o eval_results.json
```

This reads only committed files and runs in seconds. `--include-dev` also summarizes dev-split records (for example `evals/results/smoke`); `--baseline <system_id>` changes the comparison row. A scored case with no record for a system counts as a fail for that system, and the output says how many there were.

## Rerun the benchmark

You need:

- A cluster running this project (see [`../docs/setup.md`](../docs/setup.md)), with `kubectl` access from your machine and the game images for the games under test pushed.
- `GAMESERVERS_OPERATOR_CHANNEL` in the repo-root `.env`. To run several Gary cases at once, also set `GAMESERVERS_EVAL_CHANNELS` to a comma-separated list of channel ids; a Gary turn takes a per-channel lock.
- `FIREWORKS_API_KEY` in the bot's environment (for the `gary-fw` row) and in yours (for the `cc-fw` row).
- `ANTHROPIC_API_KEY` and the `claude` CLI on your `PATH` for the Claude Code rows.

```bash
# plumbing only: fake cluster, fake models, no spend
python3 evals/harness/run.py --cases evals/cases/dev --systems gary-fw,cc-fw,cc-opus --dry-run --out /tmp/dry

# one dev case, for real
python3 evals/harness/run.py --cases evals/cases/dev --systems gary-fw,cc-fw,cc-opus --case smoke-mc-pvp-d1 --out evals/results/dev

# the scored set, then the 30-case repeat subset (see evals.md §7)
python3 evals/harness/run.py --cases evals/cases/scored --systems gary-fw,cc-fw,cc-opus --concurrency 8 --out evals/results/scored
python3 evals/harness/run.py --cases evals/cases/scored --systems gary-fw,cc-fw,cc-opus --repeats 3 --case <id> ... --out evals/results/scored
```

The runner skips any job whose record is already in `--out` (pass `--force` to rerun it), so an interrupted run can be resumed. The slow-booting games (Valheim, Palworld, Enshrouded) are limited to two concurrent servers by default; change that with `--game-cap game=N`.

How each row is isolated:

- **Claude Code rows** run headless (`claude -p`) in an empty temporary directory, with a new empty config directory, no CLAUDE.md or memory, no built-in tools (`--tools ""`), and only the Gary MCP server (`--strict-mcp-config`). They get the same message, the same tools, the same tier, and the same budget (40 model calls or 20 minutes) as Gary.
- **Gary rows** run through `POST /operator/gary` with `model_profile` selecting the provider and model for that turn, and `memory: false`, so notes from one case never reach another.

## Tests

```bash
python3 -m pytest -q evals/harness/tests
```
