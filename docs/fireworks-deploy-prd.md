# Gary on Fireworks PRD

**Version:** 1.1
**Date:** 2026-11-02
**Team:** Josh (Product, Story, Presentation), Bear (Dataset, Eval, Backend), Leif (Frontend, UX)
**Event:** Fireworks Deploy Hackathon, TERRA San Francisco. Submissions close 4:30 PM PT.

---

## tl;dr

Gary is an existing AI operator that administers hosted game servers in plain English: he reads live logs and config, applies fixes, verifies the server came back healthy, and rolls back automatically if it did not. Our thesis is that a specialized operator harness running open models on Fireworks AI can do game server T1/T2 support at quality comparable to a frontier generalist (Claude Code) for a fraction of the cost and time, because the harness starts out knowing each game's config layout and quirks rather than burning turns discovering them. We ship two deliverables by 4:00 PM PT: a reproducible 4 way benchmark evaluation across 100 or more held out cases (Frontier in Claude Code vs. Open Model on Fireworks in Gary's harness vs. Open Model in Claude Code ablation vs. estimated Human T1/T2 support), and a web demo UI (chat, A2UI status and action cards, transparent tool trace, and side by side comparison).

---

## Goals

### Business Goals

- **BG-1 (Primary, Benchmark Proof, 40% of judging score):** Demonstrate on 100 or more held out cases that an open model on Fireworks inside Gary's specialized operator harness matches or beats a frontier model baseline in Claude Code on verified server recovery and config tasks, at lower cost, fewer turns/tokens, and lower p50 and p95 latency (exact multipliers populated from Bear's run, never asserted ahead of data).
- **BG-2 (Unit Economics for Hosting Providers, 25% of judging score):** Show that per incident cost drops low enough (compared against both frontier inference and estimated human T1/T2 support at Shockbyte style hosts) that a game server host can include Gary on every server rather than gating support behind slow human ticket queues.
- **BG-3 (Harness Advantage / Technical Execution, 20% of judging score):** Isolate what Gary's domain specialized operator harness contributes by running an ablation of the **same Fireworks open model inside Claude Code** alongside our system, measuring the delta in turns, tokens, latency, and game specific quirk accuracy.
- **BG-4 (Complete, Eligible Submission by 4:00 PM PT):** Ship a public, secret scrubbed GitHub repository (from Bear's clean checkpoint), evaluation evidence, and a demo video under 120 seconds before the 4:00 PM PT video cutoff and 4:30 PM PT hard close.

### User Goals

- **UG-1 (Reluctant Server Admin, "Sam"):** Hand off routine server troubleshooting ("it crashed after I added a mod", "it is lagging", "turn off PvP") to Gary so Sam stops being permanent unpaid tech support for the friend group.
- **UG-2 (Non Technical Player, "Alex"):** Ask Gary in plain English what is wrong with the server and get a working server back without reading a Java stack trace, editing a config file, or waiting hours for the admin friend to wake up.
- **UG-3 (Hackathon Judge / Hosting Evaluator, "Morgan"):** Verify in under two minutes that Gary takes real, safe actions (snapshot, edit, restart, health check, rollback), see the exact tool calls and token costs, and inspect a reproducible benchmark comparing the Fireworks model against a frontier baseline.

### Non-Goals

- **NG-1 (No new container orchestration or Agones work):** Kubernetes, Agones, the in pod supervisor, and the Discord shim already exist in `grizzly-gameservers`. We do not modify cluster infrastructure today.
- **NG-2 (No sacrificing eval rigor for game breadth):** Multi game support is a core reason the operator is an AI (handling per game config locations and quirks across games chosen by fast boot time and small image size), but game count is a dial while evaluation integrity is fixed. If time gets tight, we cut games before cutting evaluation quality or case count.
- **NG-3 (No production authentication, billing, or multi guild onboarding in the web UI):** The web frontend is a single server operator console and benchmark showcase for the demo, not a replacement for Authentik SSO or Discord guild management.
- **NG-4 (No unverified autonomous world deletion):** Destructive operations (`destroy`, `archive`, `restore`) never execute without explicit human confirmation in the UI.
- **NG-5 (No raw private data in the public repository):** No real Discord user IDs, friend messages, internal IPs, OpenBao paths, or API keys enter the public checkpoint repository.

---

## User Stories

*Convention note: Every user story below carries its own observable acceptance criteria and maps to one or more Functional Requirements (`FR-*`). Where both describe the same behavior, the Functional Requirement defines the wire contract and the story criterion defines what a user or judge sees.*

### Persona 1: Sam, the Reluctant Server Admin

Sam runs a 10 to 20 player modded Minecraft Java server for friends. Sam knows enough to rent a server, not enough to enjoy reading Fabric crash reports at 11 PM.

- **US-1 (Diagnose and fix a crash safely):** As Sam, I want to tell Gary in plain English that the server will not start after adding a mod, so that Gary reads the logs, identifies the offending or missing mod, fixes it, and brings the server back without me reading a stack trace.
  - *Acceptance Criteria:*
    - Given a server in a crashed startup state with a mod dependency or version mismatch in the log, when Sam submits `"Server won't start after I added Sodium"` in the chat panel, then within one turn Gary calls `read_logs`, identifies the failure cause in plain English, and displays every tool call in the Action Log panel.
    - When Gary modifies configuration or mods (`edit_file`, `write_file`, or `update_mods`), the Action Log visibly records the safety sequence: pre change state captured, change applied, server restart triggered, and health verification result.
- **US-2 (Gate destructive actions behind approval):** As Sam, I want any action that restores, archives, or destroys world data to pause for my explicit click, so that Gary cannot wipe our world on a misunderstanding.
  - *Acceptance Criteria:*
    - Given a prompt or failure where the recommended fix is `restore_server`, `restore_file`, `archive_server`, or `destroy_server`, when Gary proposes the fix, then the UI renders an Approval Card with `Approve` and `Deny` buttons and does not execute the destructive tool with `confirm: true` until Sam clicks `Approve`.
    - When Sam clicks `Deny`, the UI records the denial in the Action Log, makes no destructive backend call, and prompts Gary for a non destructive alternative or escalation.
- **US-3 (Trigger a reproducible failure for demonstration):** As Sam (or the demo presenter), I want to inject a known failure preset into the server with one click, so that I can test and demonstrate Gary's recovery deterministically.
  - *Acceptance Criteria:*
    - Given the demo UI is loaded, when the user selects a preset from the `Break Server` control (at minimum: Missing Mod Dependency, JVM Heap Too Small, Corrupted Config Property), then the server state transitions to the broken condition within 5 seconds and the status card reflects the unhealthy state.

### Persona 2: Alex, the Non Technical Friend

Alex plays on Sam's server, has zero server administration knowledge, and just wants to know why nobody can connect.

- **US-4 (Plain English status and read only answers):** As Alex, I want to ask `"why is the server lagging?"` or `"is the server up?"` and get a clear one paragraph answer without technical jargon or risk of breaking anything, so that I know whether to wait or play now.
  - *Acceptance Criteria:*
    - Given the caller access tier is set to `read-only` (or Alex asks a status question), when Alex submits a question in chat, then Gary invokes only read safe tools (`about_server`, `read_logs`, `server_resources`, `list_servers`) and returns a plain English summary under 120 words with an A2UI status card showing server state (`Running`, `Starting`, `Stopped`, or `Crashed`), player count, and memory usage.

### Persona 3: Morgan, the Hackathon Judge / Hosting Evaluator

Morgan is evaluating ten hackathon projects in an hour. Morgan cares about whether the open model actually works, whether the benchmark is honest (no train/test leakage), and whether the cost numbers justify switching from a frontier API.

- **US-5 (Compare Fireworks open model in Gary vs. frontier baseline and ablation):** As Morgan, I want to run the exact same task prompt against the frontier baseline (in Claude Code), our system (Fireworks open model in Gary's harness), and the ablation (same Fireworks model in Claude Code) and see their turns, tool calls, latency in milliseconds, and cost in USD side by side, so that I can judge both the open model win and what the specialized harness buys.
  - *Acceptance Criteria:*
    - Given any prompt or failure preset in the UI, when Morgan activates `Compare Models` mode, then the UI displays the comparison columns (Frontier in Claude Code vs. Fireworks in Gary, plus optional Ablation toggle) showing for each: the reply text, the list of tool calls and round count, total latency (`latency_ms`), token usage (`prompt_tokens`, `cached_tokens`, `completion_tokens`), verified outcome status, and computed cost in USD.
- **US-6 (Inspect the held out 4 way benchmark results):** As Morgan, I want to view the evaluation dashboard and run the evaluation script from the public repo, so that I can verify the 40% benchmark criterion on 100 or more held out test cases with honest reporting of measured vs. estimated numbers.
  - *Acceptance Criteria:*
    - Given the `Benchmark Results` view in the UI (and the corresponding section in `README.md`), Morgan can see: total held out test cases (`N >= 100`), the 4 comparison rows (**1. Frontier model in Claude Code**, **2. Open model on Fireworks in Gary's harness**, **3. Same open model in Claude Code ablation**, and **4. Human T1/T2 support labeled explicitly as an estimate**), verified task success rate with 95% confidence intervals, unintended side effect rate reported as its own column, turn/token efficiency, p50 and p95 latency (ms), and mean cost per 1,000 incidents (USD).
    - Running a single documented command from `README.md` reproduces the measured scoring table from the committed evaluation artifacts in under 60 seconds without requiring private cluster access.

---

## Functional Requirements

### Feature Group 1: Clean Checkpoint Repository & Fireworks Model Wiring (Priority: P0, Owner: Bear)

- **FR-1.1 (Clean Public Checkpoint Repository):** Bear creates a clean, public GitHub repository derived from `grizzly-gameservers` containing Gary's agent loop, prompt library, tool schemas, operator harness contract (`crates/control-api`, relevant `crates/bot` modules), evaluation suite, and web demo UI, with zero internal secrets, Discord snowflakes, internal domain names, or private cluster manifests.
  - *Pass/Fail:* Repository opens in a signed out incognito browser with HTTP 200; a regex credential and PII scan across the working tree and git history returns 0 hits.
- **FR-1.2 (Fireworks OpenAI Compatible Provider Support):** Configure Gary's LLM client (`crates/bot/src/agent/llm.rs` and `config.rs`, which already target `{OLLAMA_BASE_URL}/chat/completions`) to run against Fireworks AI (`https://api.fireworks.ai/inference/v1`) and a frontier baseline provider, selectable per request or via environment configuration.
  - *Pass/Fail:* A test turn via `POST /operator/gary` (or the standalone eval runner) completes a multi step tool call loop on the chosen Fireworks open model and returns populated `generations[].model`, `usage.prompt_tokens`, `usage.completion_tokens`, `latency_ms`, and `tool_calls[]`.
- **FR-1.3 (Pre existing vs. Hackathon Work Disclosure):** `README.md` includes a top level table explicitly listing what existed before 10:30 AM PT on 2026-11-02 (`grizzly-gameservers` agent loop, tool definitions, supervisor design) versus what was built during the hackathon (Fireworks integration, synthetic + seed benchmark dataset, evaluation harness, fine tuning if included, and web demo UI).

### Feature Group 2: Benchmark Dataset & Evaluation Harness (Priority: P0, Owner: Bear)

- **FR-2.1 (Task Areas, Multi Game Dial & Seed Scenarios):** Cover the two real T1/T2 hosting support workloads across games selected by fast boot time and small image size (starting with Minecraft plus additional supported `grizzly-gameservers` titles as time permits; game breadth is a dial, cutting games before cutting eval quality):
  - **Area A: Config changes & routine user support** (setting properties, enabling/disabling PvP or whitelist, installing/updating mods, tuning memory/slots without clobbering existing settings).
  - **Area B: Active troubleshooting of a broken server** across common failure modes:
    1. `eula_not_accepted`
    2. `wrong_java_or_runtime_version`
    3. `jvm_heap_oom`
    4. `port_already_in_use`
    5. `mod_missing_dependency`
    6. `mod_loader_version_mismatch`
    7. `plugin_or_mod_crash_loop`
    8. `corrupted_config_property`
    9. `corrupted_world_or_region` (requires backup/restore approval)
    10. `disk_quota_full`
    11. `entity_or_chunk_tick_lag`
    12. `whitelist_or_op_misconfig`
    13. `stale_session_lock`
    14. `resource_cap_exceeded`
    15. `not_a_server_fault` (client mismatch or benign log warning where Gary must not mutate the server)
  - Ground tasks in real support tickets and sanitized traces; keep a small dev set for building/tuning and never inspect the scored held out set while tuning (including per game knowledge notes).
- **FR-2.2 (Held Out Split & No Weakened Baseline):** Target **100 or more varied held out cases (`N >= 100`)** strictly separated from any dev cases or prompt/knowledge tuning examples. The frontier baseline receives the exact same containers, task prompts, and access in Claude Code, never a weakened setup.
  - *Pass/Fail:* Automated verification confirms zero overlap between dev cases used during building and the scored held out evaluation set (`N >= 100`).
- **FR-2.3 (4 Way Comparison & Verified Outcome Scoring):** Run and report the four comparison rows from `HACKATHON.md`:
  1. **Frontier model in Claude Code (Required Baseline):** Generic frontier agent discovering layout and fixing the server with the same container access.
  2. **Our System (Open Model on Fireworks in Gary's Harness):** Specialized operator harness with per game config/quirk knowledge served on Fireworks.
  3. **Harness Ablation (Same Fireworks Open Model in Claude Code):** Isolates what Gary's specialized harness buys vs. the raw open model (fewer turns, fewer tokens, higher success on game specific quirks).
  4. **Human T1/T2 Support (Shockbyte Style Host — Estimated):** Explicitly labeled as an **estimate** (not a measured eval run) for business value framing (ticket response latency in hours and human support cost per ticket).
  - **Verified Scoring Metrics (per measured system):**
    - **Verified Task Success Rate (% with 95% CI):** Success is verified, not claimed. The server comes back healthy AND the intended config/remediation change actually took effect.
    - **Unintended Side Effects Rate (%):** Reported as its own explicit column (for example, clobbering unrelated config lines or mutating on `not_a_server_fault`), not hidden inside the headline number.
    - **Efficiency (Turns & Tokens):** Mean rounds/turns and token consumption (`prompt_tokens`, `cached_tokens`, `completion_tokens`), showing that a specialized harness avoids exploratory wheel spinning.
    - **Latency (ms):** p50 and p95 wall clock time to verified resolution.
    - **Cost (USD per incident & per 1,000 incidents):** Computed from recorded token usage and published provider rates, reported alongside the clearly separated human T1/T2 support estimate.

### Feature Group 3: Web Demo UI — Chat, A2UI Dashboard, and Action Log (Priority: P0, Owner: Leif)

- **FR-3.1 (Chat Interface to Gary):** Provide a clean web chat panel where the user types a plain English message (or clicks one of 4 starter prompt chips) and sends it to Gary.
  - *Input contract:* Calls a thin backend proxy to `POST /operator/gary` with `{ "prompt": string, "new_session": boolean, "channel": string, "access": "admin" | "manager" | "read-only", "confirm": boolean }`.
  - *Output contract:* Renders `reply` as markdown, updates the A2UI cards, and appends `tool_calls[]`, `generations[]`, `usage`, and `latency_ms` to the Action Log and cost meter.
  - *Error state:* If the backend returns HTTP 400 or 503 (`ErrorBody { error }`), or times out after 15 seconds, the UI displays a clear inline banner and offers a one click switch to Replay Mode (FR-3.5).
- **FR-3.2 (A2UI Dashboard & Structured Cards):** Render a live server dashboard alongside chat using A2UI structured components (with a time boxed fallback rule: if A2UI library integration is not rendering cleanly by 1:00 PM PT, render identical structured JSON schemas through standard React card components so the visual and architectural story holds):
  - **Server Vital Card:** Instance name, status badge (`Running`, `Starting`, `Stopped`, `Crashed`), Minecraft edition/version + mod loader (`Fabric 1.21.1`), player count (`3 / 10`), RAM usage vs. limit (`3.2 / 4.0 GB`), and uptime/restart counter.
  - **Diagnosis & Fix Card:** Emitted on troubleshooting turns; shows detected issue title, plain English root cause, and a step list of actions taken or proposed.
  - **Approval Card:** Emitted when Gary proposes a destructive or world mutating action (`restore_server`, `restore_file`, `archive_server`, `destroy_server`); displays blast radius warning and `Approve` / `Deny` buttons.
- **FR-3.3 (Clean Action Log & Safety Loop Visualizer):** Display a live, chronological trace panel showing what Gary actually did on the turn:
  - Each `ToolTrace` entry (`name`, `arguments`, `result`) renders as a human readable step (for example, `Read server logs (last 120 lines)` or `Updated .grizzly/env: MODRINTH_PROJECTS += fabric-api`) with a toggle to expand the raw JSON arguments and verbatim dispatcher result.
  - **Safety Loop Stepper:** Whenever a turn mutates state, highlight the 4 stage guardrail progression at the top of the log: `1. Snapshot State` -> `2. Apply Fix` -> `3. Restart & Verify Ready` -> `4. Healthy (or Auto Rollback)`.
- **FR-3.4 (Per Turn Cost & Latency Telemetry Bar):** Every completed turn displays a persistent telemetry badge showing model tag (`generations[0].model`), round count (`rounds`), total latency (`latency_ms`), token breakdown (`prompt_tokens` / `cached_tokens` / `completion_tokens`), and computed turn cost in USD (to 5 decimal places, plus projected cost per 1,000 turns).
- **FR-3.5 (Deterministic Replay Mode):** Bundle pre recorded `GaryResponse` JSON traces for all demo scenarios in the frontend assets. A header toggle (`Live Cluster` / `Recorded Replay`) lets the presenter run the entire demo offline with simulated streaming latency matching the real trace's `latency_ms`.

### Feature Group 4: Model Comparison View, Fault Injection & Benchmark Tab (Priority: P1, Owner: Leif + Bear)

- **FR-4.1 (Side by Side Model & Harness Comparison Mode):** A toggle in the chat header compares the selected task across **Frontier in Claude Code**, **Open Model on Fireworks in Gary's Harness**, and the **Open Model in Claude Code Ablation** (live or from paired evaluation traces), rendering their turns, tool calls, `latency_ms`, and USD cost side by side with delta callouts (`X.Xx faster`, `Y.Yx cheaper`, `Z fewer turns`).
- **FR-4.2 (Fault Injection / "Break Server" Preset Bar):** A control bar with 3 to 4 one click presets (`Break: Missing Mod Dependency`, `Break: JVM Heap OOM`, `Break: Bad Property`, `Break: Corrupted Chunk — Needs Approval`) that either invokes `POST /operator/tool` to stage the fault on the demo instance or loads the corresponding broken server state in Replay Mode.
- **FR-4.3 (Benchmark Results Tab):** A dedicated tab in the web UI rendering Bear's locked evaluation JSON across the 4 comparison rows (with Human T1/T2 visually badged as `Estimated (Industry Baseline)` and the 3 model rows badged as `Measured on Held-Out Set`): verified success rate chart with 95% CI error bars, unintended changes column, turns/tokens per task, cost vs. latency scatter plot, and an interactive table of test cases.
  - *Shared `eval_results.json` Schema (frozen between Bear and Leif):*
    - `summary.systems[]`: `{ system_id, label, kind: "measured" | "estimated", model, harness: "gary" | "claude_code" | "human", n_cases, verified_success_rate, success_ci_95: [low, high], unintended_change_rate, mean_turns, mean_tokens, latency_p50_ms, latency_p95_ms, cost_per_1k_usd, notes }`
    - `cases[]`: `{ case_id, game, task_area: "config" | "troubleshooting", category, prompt, results_by_system: { [system_id]: { verified_success, unintended_changes, turns, latency_ms, cost_usd, tool_calls[] } } }`
  - *Shared `models.json` Rate Table:* Single file mapping model IDs to `{ input_per_1m_usd, cached_input_per_1m_usd, output_per_1m_usd }` imported by both the eval scorer and the UI telemetry bar.

### Feature Group 5: Optional Fine Tuning Pass (Priority: P2, Owner: Bear)

- **FR-5.1 (Supervised Fine Tuning or RFT on Fireworks):** If zero shot evaluation is complete before 1:30 PM PT and shows a gap in tool call formatting or category accuracy on the training split, run a fine tuning job on Fireworks using the training seed split (`train_seed_ids` only) and evaluate the tuned checkpoint as an additional row in the benchmark table.
  - *Hard Cut Rule:* If a tuned model is not trained and scored by **2:00 PM PT**, cut FR-5.1 immediately and ship the zero shot Fireworks open model versus frontier baseline comparison.

---

## User Experience

### Entry Point & First-Time Experience

- The user opens the web app URL (local dev server or static hosted build) and lands directly on the **Operator Console** view with no login wall.
- The top bar shows: product title (**Gary: Autonomous Game Server Operator**), active server selector (`mc-friends-smp · Fabric 1.21.1`), mode pill (`Live` / `Replay`), and navigation tabs (`Operator Console` | `Model Arena` | `Benchmark (N=120)`).
- Below the header sits the **Demo Scenario Bar**: 4 preset buttons (`1. Crash: Missing Fabric API`, `2. Lag: Entity Spike`, `3. World Corruption (Approval Gate)`, `4. False Alarm: Client Issue`). Clicking any preset populates the server state and suggests the player's complaint in the chat input.

### Core Experience

- **Step 1: Inject or Observe a Failure**
  - *Action:* Presenter clicks `1. Crash: Missing Fabric API`.
  - *UI Elements:* The **Server Vital Card** flips from green (`Running · 4/10 players`) to red (`Startup Crashed · Exit code 1`) and previews the last 4 lines of `latest.log` (`net.fabricmc.loader.impl.FormattedException: Mod 'iris' requires 'fabric-api'`).
  - *Validation:* Preset selection disables while a turn is in flight to prevent overlapping mutations.
  - *Navigation:* Chat input pre fills with `"Server won't start after I added Iris shaders, can you fix it?"` and focuses the `Ask Gary` button.

- **Step 2: Ask Gary & Watch the Safety Loop**
  - *Action:* User clicks `Ask Gary`.
  - *UI Elements:*
    - Center panel streams Gary's progress.
    - Right panel (**Action Log**) lights up the **Safety Loop Stepper** (`Snapshot` -> `Fix` -> `Verify` -> `Healthy`) and appends each tool call card as it completes:
      1. `read_logs` (`lines: 100`) -> finds missing `fabric-api` dependency.
      2. `search_mods` / `mod_info` (`query: "fabric-api"`) -> resolves Modrinth project slug.
      3. `edit_file` (`.grizzly/env`) -> snapshots prior overlay, adds `fabric-api` to `MODRINTH_PROJECTS`.
      4. `restart_server` -> restarts pod and waits for Agones `Ready` health check.
    - Center panel renders an **A2UI Diagnosis & Fix Card** and Gary's plain English reply: *"Iris crashed on startup because it needs Fabric API. I added Fabric API to your mod list, restarted the server, and verified it came back online."*
    - **Server Vital Card** flips back to green (`Running · Ready in 48s`).
    - Bottom of the turn displays the **Telemetry Bar**: `fireworks/glm-5p3 (or selected small model) · 2 rounds · 1,420 ms · $0.00084 ($0.84 / 1k turns)`.

- **Step 3: Approval Gate on a Risky Failure**
  - *Action:* Presenter clicks `3. World Corruption (Approval Gate)` and sends `"The server crashed while saving and won't load the world."`
  - *UI Elements:* Gary reads the logs, detects a corrupted region file, and stops before mutating. Chat renders an **A2UI Approval Card**: *"Restoring from backup `2026-11-02T10:00Z` will roll the world back 22 minutes. Approve restore?"* with `Approve Restore` (primary destructive button) and `Deny` (secondary button).
  - *Validation:* No `restore_server` call with `confirm: true` fires until `Approve Restore` is clicked.

- **Step 4: Side by Side Model Arena & Benchmark Proof**
  - *Action:* Presenter switches to `Model Arena` (or clicks `Compare with Frontier` on the completed turn), then clicks the `Benchmark (N=120)` tab.
  - *UI Elements:*
    - `Model Arena` shows the exact same turn executed by the Frontier Baseline vs. Fireworks Open Model, highlighting that both called the right tools and resolved the crash, while the Fireworks model finished faster and at a fraction of the token cost.
    - `Benchmark (N=120)` displays the 3 headline summary cards (Category Accuracy + 95% CI, Tool Rubric Score, Cost & Latency Reduction), the comparison bar charts, and the interactive test case inspector.

### Advanced Features & Edge Cases

- **Live Cluster Unreachable / WiFi Drop:** If `POST /operator/gary` fails or exceeds 15 seconds, the UI catches the error, displays a non blocking toast (`Cluster tunnel unreachable — switched to verified Replay trace`), and completes the turn from the bundled recording so a live demo never stalls.
- **False Alarm / No Op Case (`not_a_server_fault`):** When the user runs Preset 4 (`"My friend says connection timed out, is the server broken?"` while the server is healthy), Gary inspects logs and resources, confirms the server is healthy, and makes **zero** mutation tool calls. The Safety Loop Stepper badges `No Mutation Needed`.
- **Escalation Path (`escalated: true`):** When a problem is outside Gary's safe tool surface (or hits `round_budget`), the UI renders an **Escalation Card** showing `escalation_kind` and `escalation_reason` rather than a generic failure.

---

## Narrative

Sam is the only person in a twelve friend Discord who knows what SSH stands for, which means Sam is unpaid IT support every Friday night. Tonight someone wanted shaders, dropped a mod into the server, and now the Minecraft server is dead in a crash loop. In a traditional hosting setup, Sam opens a web control panel, scrolls through three hundred lines of Java stack traces on a phone screen, Googles `FormattedException`, edits the wrong config file, and loses an hour while everyone else gives up and plays something else.

With Gary, Sam does not open a config file. Sam opens the console and types: *"Server won't start after we added Iris."* On screen, Gary's action log steps through the job the way a careful site reliability engineer would. First he reads the last hundred lines of the startup log and spots the missing `fabric-api` dependency. Before touching anything, he snapshots the server's environment overlay. He adds `fabric-api`, restarts the container, and waits until the server reports healthy. Forty seconds later, the status badge turns green and Gary replies in two plain sentences.

Then comes the question every hosting company asks: why not just point a generic coding agent at the container, or use a frontier model? In the comparison view, a generalist agent in Claude Code spends extra turns hunting for where each game hides its config files and how its mod loader behaves, and a frontier model charges several cents and several seconds per incident. Right beside it, an open model on Fireworks running inside Gary's specialized harness starts out knowing the game's layout and quirks. It finishes in fewer turns, in a fraction of the time, and for fractions of a cent, and the ablation row (the exact same Fireworks model in Claude Code without Gary's harness) proves how much the specialized harness contributes. Across 100+ held out tasks in the benchmark tab, verified server recovery matches the frontier baseline within the confidence interval at a fraction of the cost of both frontier inference and human T1/T2 tickets. At that price, every hosted server can come with its own Gary.

---

## Success Metrics

### User-Centric Metrics

- **Demo Clarity Time to Value:** A judge watching the demo video or clicking through the UI sees a broken server diagnosed, safely repaired, and verified healthy in **under 45 seconds**, and completes the full product + benchmark story in **under 120 seconds** (measured by video timestamp and live rehearsal timer).
- **Zero Unsafe Mutations on Benign Prompts:** **0%** false positive mutation rate on `not_a_server_fault` test cases and **100%** approval gate enforcement on destructive tool calls (`restore_server`, `restore_file`, `archive_server`, `destroy_server`).

### Business Metrics

- **Judge Score Alignment (40% Benchmark + 25% Business Value):**
  - Held out benchmark size **N >= 120 cases** across **>= 12 failure categories**, with **0** seed overlap between train/dev and test splits.
  - Documented unit economics table showing cost per turn and cost per 1,000 turns on Fireworks vs. Frontier Baseline, translating directly into support ticket deflection economics for a server host.

### Technical Metrics

- **Failure Category Accuracy:** Open model on Fireworks achieves accuracy within the 95% bootstrap confidence interval of the frontier baseline (or exceeds the zero shot baseline after tuning) on the N >= 120 held out test set.
- **Tool Call Rubric Pass Rate:** >= 85% rubric pass rate on held out test cases (correct diagnostic tool + correct remediation tool + zero unapproved destructive calls).
- **Cost & Latency Improvement:** Measured p50 and p95 latency reduction and USD cost reduction vs. frontier baseline on the identical 120 cases (exact numbers locked by 2:30 PM PT from Bear's evaluation output).

### Tracking Plan

- **`gary_turn_completed` (Operator Harness / Eval Runner):** Recorded per turn with `model`, `rounds`, `latency_ms`, `usage.prompt_tokens`, `usage.cached_tokens`, `usage.completion_tokens`, `tool_calls[].name`, `escalated`, and `escalation_kind`.
- **`eval_case_scored` (Evaluation Harness JSON output):** Recorded per test case with `case_id`, `seed_id`, `ground_truth_category`, `predicted_category`, `category_correct`, `rubric_score`, `unsafe_action_taken`, `latency_ms`, and `cost_usd`.
- **`ui_demo_interaction` (Frontend state log):** Tracks preset selected, mode (`live` vs. `replay`), and approval button clicks (`approved` vs. `denied`) for clean video capture.

---

## Technical Considerations

### UI Architecture

- **Framework:** React (Vite + TypeScript) with Tailwind CSS for fast layout and high contrast projector readability.
- **A2UI Integration:** Render Gary's structured cards (Vital Card, Diagnosis & Fix Card, Approval Card, Escalation Card) through A2UI component schemas. **Hard fallback at 1:00 PM PT:** if A2UI package wiring costs more than 30 minutes, render the exact same card JSON schemas using local React components so Leif is never blocked.
- **Offline / Replay Resilience:** Bundle static JSON fixtures of real `GaryResponse` traces and Bear's `eval_results.json` directly into the frontend repo so the app works with zero network connectivity.

### API & Backend

- **Existing Harness Contract (`crates/bot/src/http/operator.rs`):**
  - `POST /operator/gary` accepts `GaryRequest` (`prompt`, `new_session`, `channel`, `guild`, `user`, `access`, `scope`, `confirm`) and returns `GaryResponse` (`reply`, `escalated`, `escalation_kind`, `escalation_reason`, `rounds`, `generations[]`, `usage`, `latency_ms`, `tool_calls[]`).
  - `POST /operator/tool` accepts `ToolRequest` (`tool`, `arguments`, `caller`) and returns `ToolResponse` (`tool`, `result`) for deterministic fault injection and setup.
  - `GET /operator/tools` returns the tier filtered tool schemas.
- **Model Routing (`crates/bot/src/config.rs` & `llm.rs`):** Gary already talks to an OpenAI compatible `/v1/chat/completions` endpoint configured via `OLLAMA_BASE_URL`, `OLLAMA_API_KEY`, and `OLLAMA_MODEL`. Bear points this client at Fireworks (`https://api.fireworks.ai/inference/v1`) and adds a lightweight model selector or standalone eval script so the same system prompt and tool schemas run against both the Fireworks open model and the frontier baseline.

### Performance & Scalability

- **Eval Parallelism:** Run the 120+ held out test cases with bounded async concurrency (for example, 8 to 16 concurrent requests) so a full model evaluation pass finishes in **under 4 minutes**, enabling rapid iteration before the 2:30 PM PT freeze.
- **Prompt Caching Awareness:** Preserve `cached_tokens` in `GenerationTrace` and `UsageTotals` (per ADR-042) because Gary's compiled system prompt and tool definitions are identical across turns; report both uncached and cached pricing in the benchmark.

### Integration Points

- **Fireworks AI Inference API:** OpenAI compatible chat completions with tool calling (`https://api.fireworks.ai/inference/v1/chat/completions`).
- **Frontier Baseline API:** Single frontier model endpoint used strictly as the control group on the same 120 held out test cases.
- **Clean Checkpoint Repo (Bear):** Single public GitHub repository containing the scrubbed Gary code, dataset, evaluation scripts, results JSON, and Leif's frontend.

---

## Execution Timeline & Cut Rules (Pacific Time)

| Time (PT) | Milestone | Owner | Cut / Gate Rule |
|---|---|---|---|
| **11:15 AM** | Clean checkpoint repo live; `GaryResponse` + `eval_results.json` schemas frozen | Bear + Leif + Josh | Repo must open in incognito window |
| **12:00 PM** | Fireworks model completing tool calls against Gary's prompt & schemas | Bear | Pick the best tool calling open model on Fireworks and lock it |
| **12:30 PM** | Dataset v1 generated (`N >= 120` held out test cases, seed split verified) | Bear | Run seed overlap check (`0` overlap required) |
| **12:30 PM** | UI shell rendering Chat, Vital Card, Action Log, and Safety Stepper from fixture JSON | Leif | **1:00 PM A2UI gate:** if A2UI library blocks progress, use local React cards |
| **1:30 PM** | Baseline eval runs complete (regex baseline, frontier zero shot, Fireworks zero shot) | Bear | Export `eval_results.json` for Leif's Benchmark tab |
| **2:00 PM** | UI wired to live harness + Replay Mode + Side by Side Arena | Leif | **2:00 PM Tuning gate:** if SFT/RFT is not done and winning, cut FR-5.1 and lock zero shot numbers |
| **2:30 PM** | All benchmark numbers, CIs, and charts frozen in UI and `README.md` | Bear + Leif | No code or metric changes after 3:00 PM except README polish |
| **3:00 PM** | Demo video (< 120s) recorded and verified; `README.md` complete | Josh | Video must be uploaded before 4:00 PM PT |
| **3:30 PM** | Begin HackerSquad submission; verify public repo + commit hash | Josh | Hard close at 4:30 PM PT |
