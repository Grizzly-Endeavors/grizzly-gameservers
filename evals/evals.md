# Evaluation plan

This document describes how we measure Gary against a frontier-model baseline for the Fireworks Deploy hackathon. It is written to ASD-STE100 (Simplified Technical English) rules.

## 1. Purpose

The evaluation answers one question: can an open model in Gary's harness complete game-server support tickets as well as a frontier model in a general agent, at a lower cost and in less time?

A "ticket" is a request that a customer sends to the support team of a game-server host. Each ticket in the evaluation is a type of ticket that T1/T2 support at a host such as Shockbyte must handle.

The evaluation measures task completion only. It does not measure escalation, refusal, or safety behavior.

## 2. Systems

We compare these systems. All measured systems use the same cases, the same tools, and the same scoring.

| Row | System | Model | Harness | Type |
|---|---|---|---|---|
| 1 | Frontier baseline | Claude Opus 5.5 | Claude Code | Measured |
| 2 | Our system | Open model on Fireworks | Gary | Measured |
| 3 | Harness ablation | The same open model as row 2 | Claude Code | Measured |
| 4 | Frontier in Gary (optional) | Claude Opus 5.5 | Gary | Measured |
| 5 | Human T1/T2 support | Not applicable | Not applicable | Estimate |

Row 3 shows what the harness contributes. Row 4 shows what the model contributes. Row 5 is an estimate from published data. We do not put row 5 in the same table as the measured rows.

### 2.1 Same access for all systems

- All systems use Gary's tool surface. The Claude Code rows get these tools through an MCP server that calls the operator tool route.
- All systems get the same customer message for each case.
- All systems get the same budget: 40 model calls or 20 minutes for each case. The first limit that the system reaches stops the case. A stopped case is a failure.
- Gary's guardrails stay on for all systems. We do not score them.

### 2.2 Isolation of the Claude Code rows

- Run Claude Code in headless mode with a new, empty configuration directory. No user or project instruction files load.
- Start Claude Code in an empty temporary directory. It must not see the scenario catalog, the checks, or the repository.
- Give Claude Code the Gary MCP server only. Disable its local shell and file tools.

### 2.3 Isolation of the Gary rows

- Start each case with an empty Gary memory. Notes from one case must not go into a different case.

## 3. Scenario set

The scenario catalog is `evals/scenarios.yaml`. Each scenario is a template. The case generator expands each template into variants. A variant is one concrete case.

### 3.1 Size

| Game | Scored cases |
|---|---|
| Factorio | 24 |
| Minecraft (Fabric) | 20 |
| Valheim (BepInEx) | 17 |
| Terraria (TShock) | 16 |
| Palworld | 14 |
| Enshrouded | 11 |
| **Total** | **102** |

There are also 36 development cases. One development case exists for each of the 35 scenarios in the catalog. The 36th is `smoke-mc-pvp`, a smoke case that is not in the catalog.

### 3.2 Ticket types

| Type | Meaning |
|---|---|
| config | Change a setting, for example a password, a player limit, or a difficulty. |
| fix | Repair a server that does not start or does not operate correctly. |
| world | Load an uploaded world or save, or make a new world. |
| access | Add an admin, a whitelist entry, or a password group. |
| mods | Install, update, share, or import mods. |

### 3.3 Sources

Each scenario has a `source` field. This field gives the host knowledge-base article that the ticket type comes from. Shockbyte blocked automatic reads of its site. Thus, the sources are article titles and search-result text, not the full articles. Where Shockbyte had no article, the source is a different host.

### 3.4 Dev and scored split

- The split is by variant. A scored case does not share its staging values with a development case. Staging values include the mod, the setting value, the world name, and the customer message.
- Use only the development cases to tune prompts, game knowledge, and the harness.
- Do not look at the scored cases or their results until the freeze.
- Before the scored run, do an automatic overlap check. The result must show zero overlap.

### 3.5 Scenarios that the tools possibly cannot do

Two scenarios with the mark `reach: verify` keep their scored cases: `vh-upload-world` (2 cases) and `fa-load-save` (3 cases). The current tools possibly cannot do these fixes. The staging test examines each one:

- If a small change to the tools makes the fix possible, we make the change before the freeze. The change applies to all systems.
- If not, we remove the scenario. We then add cases from other scenarios to keep the scored set at 100 or more.

Six scenarios are dropped. They have no cases, and `scenarios.yaml` marks each one `dropped`:

- `tr-admin`: Gary's tools cannot change TShock groups.
- `fa-space-age`: the image force-enables the expansion on every start, so the broken state cannot be staged.
- `pw-settings-ignored`: a real `WorldOption.sav` cannot be made without a game client.
- `en-transfer-world`: there is no file move or copy tool (issue #129).
- `mc-plugins-on-modded` and `mc-upgrade-version`: not staged in time.

The scored set is 102 cases. This meets the 100 target in the rule above.

## 4. Procedure for one case

1. Start a server for the game from a known baseline.
2. Apply the staging recipe with the operator tool route. No model is used in this step.
3. Wait for the broken condition that the scenario expects. If the condition does not occur, the case is not valid. Record it as a staging error, not as a system failure.
4. Send the customer message to the system.
5. Let the system operate until it stops, or until it reaches the budget.
6. Wait for the server to become Ready. Use the normal boot time for that game as the limit.
7. Run the scenario checks.
8. Restart the server one more time. Wait for Ready. Run the checks again.
9. Record the result and the measurements.
10. Remove the server.

## 5. Pass criteria

A case passes when all of these conditions are true:

- The server is Ready after the system stops.
- Each scenario check passes.
- The server is Ready again after the second restart, and each check passes again.

The checks examine only what the ticket asks for. A check can also examine a result that the ticket clearly implies. For example, a crash fix must keep the customer's other mods. The checks do not examine how the system did the fix. The checks do not examine other changes to the files.

The runner decides the result. The statement of the system ("it is fixed") does not decide the result.

## 6. Measurements

### 6.1 Headline measurements

| Measurement | Definition |
|---|---|
| Success rate | The number of cases that pass, divided by the number of scored cases. |
| Cost | The mean cost in USD for each case, and the cost for 1,000 cases. |
| Time | The wall-clock time from the customer message to the end of the system's work. We give the p50 and the p95. |

We calculate cost from the token counts. We use one rate table (`models.json`) for all systems. The rate table has a rate for input tokens, cached input tokens, and output tokens. We keep the cost report from Claude Code only as a cross-check.

### 6.2 Supporting measurements

- Success rate for each game and for each ticket type.
- Number of model calls and tool calls for each case.
- Tokens for each case: uncached input, cached input, and output.
- Time for each case, divided into model time and tool time.
- False completion rate: the number of cases where the system said that the task is done but the checks failed. Infra errors are not counted.

## 7. Statistics

- Give a 95% Wilson interval for each success rate.
- Compare each system to the frontier baseline on the same cases. Use the McNemar test for the difference in success.
- Use a paired bootstrap to give 95% intervals for the ratios of cost and time.
- Run each scored case one time for each system.
- Run a subset of 30 scored cases three times for each system. Use these runs to show the variation between runs.

## 8. Integrity rules

- All measured systems use the same scored cases and the same scoring.
- Do not make the baseline weaker. It gets the same tools, messages, budget, and server state.
- Tune only on the development cases.
- Report all scored cases. Do not remove failed cases from the results.
- Keep measured results and estimates separate.
- Do not put credentials, personal data, or data from real friend servers in the repository.

## 9. Limits

- A setting is "in effect" when it stays in the file that the game reads after a restart. We do not join the game to see the setting.
- The evaluation does not measure escalation, refusal, or safety.
- We run each scored case only one time. The 30-case subset gives the variation between runs.
- The sources for the tickets are article titles and search-result text, not full articles.
- The human support row is an estimate.

## 10. Output

The runner writes `eval_results.json` in the schema that the PRD specifies (`docs/fireworks-deploy-prd.md`, FR-4.3). The web UI and the README use this file. A single command recalculates the tables from the committed results. This command does not need access to the cluster.
