# grizzly-gameservers

**An LLM ops agent — Gary — that operates live production systems unattended, and the game-server platform that gives him something real to run.**

Gary reads a running server's actual config and logs, makes the change, restarts, verifies it worked, and rolls it back if it didn't. Non-technical friends ask him in plain English in the Discord channel they already use, or from inside the game itself; slash commands sit alongside him for the mechanical lifecycle actions. Servers run as containers in the homelab Kubernetes cluster (via Agones), exposed through the Hetzner proxy VPS.

Game servers are the domain, not the point — they're a proving ground for safe autonomous mutation, with genuine per-game variety, non-technical users typing arbitrary free text, and a blast radius that's a game world rather than a payments ledger. Gary is the product; `grizzly-gameservers` is the project that houses him, and every identifier here keeps that name ([ADR-028](docs/decisions/028-gary-is-the-product.md)).

See [`docs/design/00-overview.md`](docs/design/00-overview.md) for the architecture, [`docs/product/what-this-is.md`](docs/product/what-this-is.md) for the plain-language version, and [`CLAUDE.md`](CLAUDE.md) for working conventions. **Status: live.** Gary, the in-pod process supervisor, and the Discord shim are all deployed and gate-signed via CI; see [`docs/activation-status.md`](docs/activation-status.md) for what's verified.

Gary records what he does to two data planes — his turns to Langfuse, product usage to Postgres. [`docs/operations/data-collection.md`](docs/operations/data-collection.md) is how to get numbers out of them.

A Cargo workspace with four crates: `crates/bot` (Gary's agent loop + the Discord shim he's reached through + Agones client, binary `grizzly-gameservers`), `crates/supervisor` (the in-pod process supervisor baked into game images — Gary's hands inside a pod, binary `grizzly-supervisor`), `crates/control-api` (the wire contract shared by the two — request/response bodies plus the route-path constants and `CONTROL_PORT`, so neither side hardcodes a bare literal like `9359` or `/fs/read`), and `crates/prompt-lib` (build-time compiler that turns the bot's `prompts/` Markdown tree into typed Rust accessors).

## Build

```bash
cargo build --workspace
cargo build --release --workspace
```

## Run

Run the Discord bot against your current kubeconfig context (reads secrets from a repo-root `.env`):

```bash
scripts/local-bot.sh start   # or: just bot-start
```

The supervisor runs inside game-server pods, not locally — it's built into the per-game image (`games/<game>/Dockerfile`).

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

Pre-commit and commit-msg hooks live under `.githooks/`. Install with:

```bash
./.githooks/install.sh
```

They enforce formatting, clippy, tests, and conventional commit message format. Bypass is not supported.
