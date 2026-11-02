# Gary eval: how to reproduce it

This directory holds the evaluation that compares Gary (an open model on Fireworks, in the Gary harness) with a frontier model in Claude Code on game-server support tickets. The plan, the pass criteria, and the statistics are in [evals.md](evals.md). This file tells you how to run it again.

## Reproducing these results

Tiers 1 to 3 need no cluster and no paid model calls. Tier 4 is the only one that needs a cluster, model keys, and real spend.

1. **Re-score with no cluster (under a minute).** This rebuilds every table and `eval_results.json` from the committed run records. It needs Python 3 and `pyyaml`.

   ```
   pip install pyyaml
   python3 evals/harness/score.py evals/results/<scored-run> --cases evals/cases/scored --human-estimate evals/human_estimate.json -o evals/results/<scored-run>/eval_results.json > evals/results/<scored-run>/summary.md
   ```

2. **Inspect any case.** Every run record is `evals/results/<run>/<system>__<case>__r<N>.json`, and its transcript is in `evals/results/<run>/transcripts/`. A Gary turn's `<stem>.messages.json` holds the full model conversation, including each tool call and its result, and the run record holds the token usage (prompt, cached, and completion). A Claude Code row keeps the final `claude -p` JSON output (reply, usage, cost) in its transcript and the tool names in its run record; the individual tool results of a Claude Code turn are not captured. `evals/harness/scrub.py` replaces Discord snowflakes with `<discord-id>` when each file is written, and nothing else is edited. Every record the runner wrote is committed, including failed cases, staging errors, and infra errors. A rerun of those errors (`--rerun-outcomes`) or a `--force` rerun replaces the record in place, so the earlier attempt survives only in git history.

3. **Dry-run the harness (no cluster, no spend).** The dry run uses a fake cluster and fake systems. Pass `--out`: without it, a dry run writes to `evals/results/runs/`.

   ```
   python3 evals/harness/run.py --cases evals/cases/scored --systems gary-fw,cc-fw,cc-opus --dry-run --out /tmp/dry
   python3 -m unittest discover -s evals/harness/tests -q
   ```

4. **Full rerun (needs a cluster and spend).** You need:
   - A Kubernetes cluster running this project with Agones. [docs/setup.md](../docs/setup.md) covers standing it up.
   - Game images built from `games/<game>/Dockerfile` for each game the scored cases use. `scripts/push-game-image.sh [game] [tag]` builds and pushes one.
   - A Fireworks key (`FIREWORKS_API_KEY`) and an Anthropic key (`ANTHROPIC_API_KEY`) in the repo-root `.env`, as listed under [Prerequisites](#prerequisites).
   - The scored command under [Commands](#commands), with `--concurrency 20`.

   The servers are real game servers, and the model calls are billed. A full scored pass takes about 1 to 2 hours of wall clock on our cluster at `--concurrency 20`, for all three systems at one repeat.

## What is measured

Each case is one support ticket: a customer message plus a server staged into a broken or baseline state. A system gets the message, operates the server through Gary's tool surface, and stops. The runner then checks the server itself. The system's own claim ("it is fixed") does not count. A case passes only when the server is Ready, every check passes, and both still hold after one more runner-triggered restart.

The headline numbers are verified success rate (with a 95% Wilson interval), mean cost per case and per 1,000 cases, and time from the customer message to the end of the work (p50 and p95). The eval measures task completion only. It does not measure escalation, refusal, or safety behavior. See [evals.md](evals.md) sections 5 to 7.

## Systems compared

The definitions are in `harness/systems.json`. Rates are in `harness/models.json`.

| System id | Model | Harness | Role |
|---|---|---|---|
| `cc-opus` | Claude Opus 5.5 | Claude Code | Frontier baseline |
| `gary-fw` | GLM 5.3 Flash on Fireworks | Gary | Our system |
| `cc-fw` | GLM 5.3 Flash on Fireworks | Claude Code | Harness ablation (same model as `gary-fw`) |
| `gary-ollama-dev` | GLM 5.2 on Ollama | Gary | Dev smoke only; not a result row |
| `human-t1t2` | Not applicable | Not applicable | Estimate from `human_estimate.json`; never measured |

All measured systems get the same cases, the same customer message, the same tool surface (the Claude Code rows reach it through the MCP server in `harness/gary_mcp.py`), and the same budget of 40 model calls or 20 minutes per case. Gary starts each case with its memory turned off. See [evals.md](evals.md) section 2.

The Claude Code rows run `claude -p` in an empty temporary directory with a fresh, empty `CLAUDE_CONFIG_DIR`, so no `CLAUDE.md`, memory, settings, skills or plugins load. Built-in tools are disabled (`--tools ""`), and `--strict-mcp-config` makes `gary_mcp.py` the only MCP server, with `remember`/`forget` excluded so neither side has durable memory. The runner records whether `--bare` mode ran.

## Layout

| Path | Content |
|---|---|
| `evals.md` | The evaluation plan (pass criteria, measurements, statistics, integrity rules). |
| `scenarios.yaml` | The scenario catalog: one template per ticket type, with the dev and scored variant counts. |
| `cases/dev/`, `cases/scored/` | One YAML file per concrete case. Schema: [cases/README.md](cases/README.md). |
| `fixtures/` | World and save archives that some cases upload. |
| `harness/run.py` | Stages, runs, and grades cases on live servers; writes one run record per (system, case, repeat). |
| `harness/score.py` | Turns run records into `eval_results.json` and a markdown table. |
| `harness/check_split.py` | Checks that the dev and scored sets do not overlap. |
| `harness/export_sft.py` | Exports dev-split Gary turns as Fireworks fine-tuning JSONL. |
| `harness/gary_mcp.py` | MCP server that exposes Gary's tools to Claude Code. |
| `harness/tests/` | The harness's `unittest` suite. |
| `human_estimate.json` | The estimated human support row, with sources. |
| `results/` | Run records, transcripts, and scored output. |
| `DISCLOSURE.md` | What existed before the event and what was built during it. |

## Prerequisites

- Python 3 with `pyyaml`.
- A cluster running the Gary bot and Agones; [docs/setup.md](../docs/setup.md) covers standing one up.
- `kubectl` access to the cluster that runs the Gary bot and Agones. The runner port-forwards the bot's operator route and reads the operator bearer token from the `grizzly-gameservers-operator` Secret in memory. It never prints the token. To skip the Secret read, set `GAMESERVERS_OPERATOR_TOKEN`.
- A repo-root `.env` (gitignored) with these keys. Names only; never commit values:
  - `GAMESERVERS_OPERATOR_CHANNEL`: the Discord channel that receives the operator audit lines. `GAMESERVERS_EVAL_CHANNELS` (comma list) may add more channels for parallel Gary turns.
  - `FIREWORKS_API_KEY`: needed for `gary-fw`, and for `cc-fw` (the runner passes it to Claude Code as its auth token).
  - `ANTHROPIC_API_KEY`: needed for `cc-opus`.
- A deployed bot that accepts the per-turn `model_profile` and `memory` fields on the operator route. The Gary rows need both.
- Namespace quota for the parallel fleet. Each case creates a fresh server and always destroys it.

Scoring and splitting need only Python. They need no cluster.

## Commands

Run from the repo root.

**Check the split first.** The result must show zero overlap and counts that match `scenarios.yaml`.

```
python3 evals/harness/check_split.py
```

**Rehearse without a cluster or paid calls:** see [Reproducing these results](#reproducing-these-results), tier 3.

**Dev run.** Use the dev cases to tune prompts, game knowledge, and the harness. Do not use the scored cases for this.

```
python3 evals/harness/run.py --cases evals/cases/dev --systems gary-fw,cc-fw,cc-opus --out evals/results/dev-1
```

`evals/cases/dev/smoke-mc-pvp-d1.yaml` is a one-case smoke check (`--case smoke-mc-pvp-d1`). The runner skips a (system, case, repeat) whose record already exists in the output directory; pass `--force` to rerun it.

**Scored run.** Run only after the freeze. One run per case per system; `--repeats 3` on a chosen subset (use `--case ID` for each) gives the variation between runs. Without `--out`, records go to `evals/results/runs/<UTC timestamp>/`. `--concurrency` sets parallel servers and `--game-cap game=N` limits servers per game.

```
python3 evals/harness/run.py --cases evals/cases/scored --systems cc-opus,gary-fw,cc-fw --out evals/results/scored-1
```

**Score.** Each run record is one JSON file. `-o` writes the schema in `docs/fireworks-deploy-prd.md` (FR-4.3). The markdown table goes to stdout.

```
python3 evals/harness/score.py evals/results/scored-1 --cases evals/cases/scored --human-estimate evals/human_estimate.json -o evals/results/scored-1/eval_results.json > evals/results/scored-1/summary.md
```

Pass `--baseline <system_id>` to pick the comparison baseline (default: the Claude Code Opus row), or `--no-baseline` to skip comparisons. Pass `--include-dev` to score dev records, which are excluded by default.

**Recompute the tables from committed results, with no cluster access:** see tier 1 of [Reproducing these results](#reproducing-these-results).

**Export fine-tuning data (dev only).** The export keeps dev-split Gary turns and aborts before writing anything if an input holds a scored record.

```
python3 evals/harness/export_sft.py evals/results/dev-1 -o train.jsonl [--only-pass]
```

**Run the harness tests:**

```
python3 -m unittest discover -s evals/harness/tests -q
```

## Where results land

Each run directory holds `<system>__<case>__r<N>.json` run records and a `transcripts/` directory. Gary transcripts hold the model conversation and every tool call and result; Claude Code transcripts hold the final `claude -p` output. Scored output is `eval_results.json` (systems summary, then per-case results) and `summary.md`. `results/smoke/` holds the records from the first paid smoke run on one dev case.

Run records do not yet carry a manifest. Nothing in a run directory records the `systems.json` or `models.json` a run used, the eval repo commit, or the bot image digest the cluster ran. To recover them, use the commit that added the run directory and the bot digest pinned in `deploy/values.yaml` at that commit.

## Integrity rules

- **Dev and scored split.** The split is by variant. A scored case shares no staging value (mod, setting value, world name) and no customer message with a dev case. Only dev cases are used to tune prompts, game knowledge, and the harness.
- **Overlap check.** `check_split.py` fails on a shared id, a shared prompt, a shared scenario plus identical setup, a case in the wrong split directory, or a dev or scored count that disagrees with `scenarios.yaml`. Run it before the scored run.
- **Scored data is never training data.** `export_sft.py` accepts dev records only and refuses any input that holds a scored record.
- **No weaker baseline.** The frontier baseline gets the same tools, messages, budget, and server state as every other system.
- **Everything is reported.** Failed, stopped, and missing cases count as failures in the scored table. Infra errors and staging errors are reported separately and are not blamed on a system.
- **Measured and estimated stay apart.** The human row has `kind: estimated` and is never mixed into the measured rows.

## Cost and budget notes

- One rate table, `harness/models.json`, prices every system from recorded token counts (input, cached input, output; USD per 1M tokens). The cost that Claude Code reports is only a cross-check.
- Claude Opus 5.5: $4.00 input, $0.20 cached input, $20.00 output. Cache writes are billed at $5 per 1M tokens but counted at the $4 input rate, so the baseline cost is slightly undercounted.
- GLM 5.3 Flash on Fireworks: $0.15 input, $0.03 cached input, $0.50 output. Rates are from the Fireworks serverless pricing page as of 2026-11-02.
- GLM 5.2 on Ollama is a flat subscription, so its marginal cost counts as zero. That row is for dev smoke runs only.
- Each case is capped at 40 model calls or 20 minutes. The first limit reached stops the case, and a stopped case is a failure.

## Known limitations

- The human row is an estimate, not a measurement. It uses IT-helpdesk benchmarks from 2016 that are not inflation-adjusted, not specific to game hosting, and with an assumed 70/30 T1/T2 split. Time to resolution is our judgement. See `human_estimate.json`.
- The ticket types come from host knowledge-base article titles and search-result text, because the host's site blocked automatic reads. The tickets are realistic, not copied from real customers.
- Cases check what the ticket asks for, not how the fix was done or what else changed. Collateral damage and unneeded changes are not scored.
- Only task completion is scored. A system that should escalate or refuse is not rewarded for doing so.
- Each scored case runs once per system; only a chosen subset runs three times, so run-to-run variation is shown for that subset only.
- Some scenarios are marked `reach: verify`: the tool surface may not be able to perform the fix. Staging decides whether the tools are widened (for all systems) or the scenario is dropped.
- The eval needs a live cluster and paid model calls. Scoring from committed run records does not.

## Results

The results table is generated by `harness/score.py` into `evals/results/<run>/eval_results.json` and `summary.md`.
