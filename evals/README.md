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

The scored run is `evals/results/scored-3/`. It was stopped at 15:40 PT so it could be submitted; it did not finish the planned set. All 89 records it wrote are committed, including failures and staging errors.

### Headline: cases graded for all three systems (N = 14)

`evals/results/scored-3-matched/` holds the 14 scored cases that reached a pass or fail on every system (4 Factorio, 4 Minecraft, 3 Terraria, 2 Valheim, 1 Enshrouded). Re-score with `python3 evals/harness/score.py evals/results/scored-3-matched --cases evals/results/scored-3-matched/cases --human-estimate evals/human_estimate.json -o /tmp/eval_results.json`.

| System | Success (95% CI) | Cost per 1,000 tickets | p50 time | p95 time | Mean model turns |
|---|---|---|---|---|---|
| Claude Opus 5.5 in Claude Code (baseline) | 78.6% [52.4%, 92.4%] | $97.04 | 66 s | 120 s | 10.4 |
| GLM 5.3 Flash on Fireworks in Gary | 71.4% [45.4%, 88.3%] | $7.19 | 87 s | 612 s | 9.4 |
| GLM 5.3 Flash on Fireworks in Claude Code (ablation) | 78.6% [52.4%, 92.4%] | $5.36 | 34 s | 116 s | 11.6 |
| Human T1/T2 support (**estimate**, not measured) | n/a | $48,305 | 24 h | 5 days | n/a |

- **Cost.** Gary on GLM 5.3 Flash costs 0.07× the baseline (95% CI 0.05–0.11), about 13× cheaper, and about 6,700× cheaper than the estimated human ticket.
- **Success.** Gary is 1 case behind the baseline (10 vs 11 of 14). McNemar p = 1.0: no significant difference at this N. The intervals are wide.
- **Time.** Gary is slower than the baseline at p50 (2.1×, CI 1.0–3.6) and has a long tail. Gary's restart tool waits for the server to come back and confirms the change held before the turn ends; the Claude Code rows do not wait for that. Every measured system answers in minutes against an estimated 24-hour human median.

### Pairwise, larger N

The three-way set is small because a staging error on any one system drops the case for all. Pairwise against the baseline, on cases both systems graded:

| Pair | Paired cases | System passes | Baseline passes |
|---|---|---|---|
| Gary (GLM) vs Opus in Claude Code | 17 | 12 | 13 |
| GLM in Claude Code vs Opus in Claude Code | 15 | 12 | 12 |

Graded-only rates over everything each system finished: Opus 20/28, Gary 12/17, GLM in Claude Code 13/16. `evals/results/scored-3/summary.md` is the scorer's full output, which counts the cases a system never reached as fails; read its success column with that in mind.

### What the ablation shows

Both Claude Code rows used Gary's tool surface through MCP: `game_guide`, file edits that snapshot first, restart with verify and auto-rollback, the mod-list tools. That is the strongest baseline we could give Claude Code, not a weakened one. With the tools held equal, GLM in Claude Code matches Opus, so most of what lets a cheap open model do this work is Gary's tool surface rather than his agent loop. A baseline with generic shell access only (no Gary tools) is built on the local Docker backend (`cc-opus-generic`) but was not run on the scored set before the deadline.

### Generic-shell baseline (13 of the 14 matched cases)

`evals/results/generic-shell-opus/` holds `cc-opus-generic`: Opus 5.5 in Claude Code with no Gary tools, only `docker exec`, `docker logs` and `docker restart` on the server's container, run on the local Docker backend against the same matched cases. `vh-import-modpack-s1` did not finish before the deadline and is left out of every row below.

| System (same 13 cases) | Passed | Cost per 1,000 tickets | p50 time | Mean turns |
|---|---|---|---|---|
| Opus 5.5, generic shell only | 13/13 | $94.13 | 82 s | 8.2 |
| Opus 5.5 + Gary's tools (Claude Code) | 11/13 | $97.98 | 66 s | 10.3 |
| GLM 5.3 Flash in Gary | 10/13 | $7.54 | 92 s | 9.7 |
| GLM 5.3 Flash + Gary's tools (Claude Code) | 11/13 | $5.56 | 34 s | 11.9 |

A frontier model with a raw shell solves these tickets without Gary's tools. Gary's result is economic: GLM 5.3 Flash in Gary resolves 10 of the 13 at about 1/12th the cost of generic Opus. The generic run is on Docker rather than the cluster, so it is not a strict pair with the other rows. The run surfaced a harness bug (a `docker restart` republishes the control port, so the runner polled a stale address); the fix is on `feat/cc-generic`, and every case it affected was rerun.

### Limitations of this run

- **Small N.** 14 three-way cases and 15–17 pairwise, against a target of 100+. The cluster's storage (a ZFS pool on the R730xd, provisioned over SSH by democratic-csi) saturated on game-server boots and capped how many servers could start; 27 cases timed out booting (staging errors, excluded from their system's denominator and from the matched set).
- **Grader defects found during the run.** Three checks failed correct fixes on every system: Factorio's `mod-list.json` check demanded an explicit entry for a mod Factorio leaves unlisted, one admin check was case-sensitive where Factorio lowercases names, and post-restart checks ran before the game process was back. All three are fixed in the case files and the grader, but some records were graded before the fixes, and those false fails stay in the results. They affect all systems.
- **Two aborted runs** are committed beside this one: `aborted-scored-1-stale-images` (game images had an old supervisor, so file tools failed) and `aborted-scored-2-overlap` (two runners shared server names). Neither is scored.
- **Single-turn tickets.** A system that asks the customer a clarifying question instead of acting fails the case.
