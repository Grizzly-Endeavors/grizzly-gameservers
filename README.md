# grizzly-gameservers

**An LLM ops agent — Gary — that operates live production systems unattended, and the game-server platform that gives him something real to run.**

Gary reads a running server's actual config and logs, makes the change, restarts, verifies it worked, and rolls it back if it didn't. Non-technical friends ask him in plain English in the Discord channel they already use, or from inside the game itself; slash commands sit alongside him for the mechanical lifecycle actions. Servers run as containers in the homelab Kubernetes cluster (via Agones), exposed through the Hetzner proxy VPS.

Game servers are the domain, not the point — they're a proving ground for safe autonomous mutation, with genuine per-game variety, non-technical users typing arbitrary free text, and a blast radius that's a game world rather than a payments ledger. Gary is the product; `grizzly-gameservers` is the project that houses him, and every identifier here keeps that name ([ADR-028](docs/decisions/028-gary-is-the-product.md)).

See [`docs/design/00-overview.md`](docs/design/00-overview.md) for the architecture, [`docs/product/what-this-is.md`](docs/product/what-this-is.md) for the plain-language version, [`docs/setup.md`](docs/setup.md) for how to stand it up, and [`CLAUDE.md`](CLAUDE.md) for working conventions. **Status: live.** Gary, the in-pod process supervisor, and the Discord shim are deployed through CI and Flux; see [`docs/activation-status.md`](docs/activation-status.md) for what's verified.

## Fireworks Deploy hackathon (2026-11-02)

**Task:** game-server T1/T2 support tickets ("add this mod", "the server won't start since the update", "make my friend an admin"), worked end to end on real servers. **Thesis:** an open model on Fireworks inside Gary's specialized harness can match a frontier generalist on verified task success, at a fraction of the cost. **Baseline:** Claude Opus 5.5 in Claude Code, given exactly Gary's tools through MCP. **Ablation:** the same open model in Claude Code, which isolates what the harness contributes. The eval plan is [`evals/evals.md`](evals/evals.md), and the run and re-score instructions are in [`evals/README.md`](evals/README.md).

### Pre-existing work vs. built during the event

Gary existed before the event and already ran live game servers for a small group of friends. Everything up to the [`pre-hackathon`](https://github.com/Grizzly-Endeavors/grizzly-gameservers/tree/pre-hackathon) branch (commit `878564d`, 2026-10-15) is prior work. Every commit after it was made during the event (from 10:30 AM PT on 2026-11-02). To see the full hackathon diff, run `git diff pre-hackathon main`, or open [the compare view](https://github.com/Grizzly-Endeavors/grizzly-gameservers/compare/pre-hackathon...main).

| Existed before (on `pre-hackathon`) | Built during the event |
|---|---|
| Gary's agent loop, tool set, prompts, guardrails (snapshot → apply → verify → auto-rollback, escalation), Discord shim | Fireworks as a per-turn model provider, plus memory-off turns, on the operator harness (`crates/bot`) |
| The per-pod supervisor and its control API (`crates/supervisor`, `crates/control-api`) | Fuller operator-turn capture (reasoning, tools, raw messages) and fine-tuning JSONL export |
| Game catalog for 7 games, Agones and cluster guardrails, Helm chart, CI | Valheim world and Factorio save switching for each server (needed by eval tickets) |
| Operator harness (`/operator/*`, `just operator`), Langfuse telemetry, product event log | The whole eval: scenario catalog, dev and scored cases, fixtures, runner, scorer, Gary-as-MCP server for Claude Code (`evals/`) |
| 42 ADRs and the design docs | Web console demo UI (`web/`); its shell and design tokens were adapted from an earlier internal project of a team member |
| | PRD, pitch deck, eval plan (`docs/fireworks-deploy-*`, `docs/pitch-deck.html`, `evals/evals.md`) |

**AI assistance:** most of the code, before and during the event, was written with Claude Code under the team's direction and review. The repo's conventions for agents are in [`CLAUDE.md`](CLAUDE.md) and [`AGENTS.md`](AGENTS.md).

**Team:** Bear Flinn, Josh Flinn, Leif Campbell.

Gary records what he does to two data planes — his turns to Langfuse, product usage to Postgres. [`docs/operations/data-collection.md`](https://github.com/Grizzly-Endeavors/grizzly-gameservers/blob/pre-hackathon/docs/operations/data-collection.md) is how to get numbers out of them.

A Cargo workspace with three crates: `crates/bot` (Gary's agent loop + the Discord shim he's reached through + Agones client, binary `grizzly-gameservers`), `crates/supervisor` (the in-pod process supervisor baked into game images — Gary's hands inside a pod, binary `grizzly-supervisor`), and `crates/control-api` (the wire contract shared by the two — request/response bodies plus the route-path constants and `CONTROL_PORT`, so neither side hardcodes a bare literal like `9359` or `/fs/read`). The bot's `prompts/` Markdown tree is compiled into typed Rust accessors by [`grizzly-agent-prompts`](https://github.com/Grizzly-Endeavors/grizzly-agent), a build/dev dependency rather than a workspace member.

## Build

```bash
cargo build --workspace
cargo build --release --workspace
```

## Run

The bot runs as a Deployment in the cluster and the supervisor runs inside each game-server pod; [`docs/setup.md`](docs/setup.md) covers the full deployment and what runs without a cluster. To iterate off-cluster, `scripts/local-bot.sh start` (or `just bot-start`) runs the bot against your current kubeconfig context, reading secrets from a repo-root `.env`.

## Test

```bash
cargo test --quiet --all-features
```

## Dev Tasks

Via `just` (convenience, not required):

- `just` / `just ci-local` — fmt-check, lint, test, deny
- `just test` — `cargo test --quiet --all-features`
- `just fmt` — `cargo fmt --all`
- `just lint` — `cargo clippy --all-targets --all-features -- -D warnings`
- `just deny` — `cargo deny check`

## Git Hooks

Install once per clone with `just hooks` (or `./.githooks/install.sh`). A push to `main` deploys the bot only when it touches an input of the bot image (`crates/` outside `tests/`, `games/`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `Dockerfile`, `.dockerignore`, or `deploy.yml`; the `paths:` filter in `.github/workflows/deploy.yml`), so docs, evals, web, cluster, scripts and test-only pushes leave the live pod alone. The hooks are the check before code goes live:

- **pre-commit** (a few seconds): `cargo fmt --check`, clippy, and no `dbg!` or `#[allow]` in staged Rust.
- **pre-push**: the tests of the crates the pushed commits touch (`.githooks/lib/select-checks.sh` maps paths to crates), plus offline `cargo deny` when dependencies change. Docs-only pushes run nothing.
- **commit-msg**: conventional commit format.

Bypass is not supported.
