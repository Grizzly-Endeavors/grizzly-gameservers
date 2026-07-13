# grizzly-gameservers

A Discord-driven service for spinning up and managing game servers on Grizzly Endeavors hardware. Friends issue commands in Discord; servers run as containers in the homelab Kubernetes cluster (via Agones) and are exposed through the Hetzner proxy VPS. An LLM ops agent handles per-game config tweaks and break-fixing so day-to-day operation doesn't need a human.

See [`docs/design/00-overview.md`](docs/design/00-overview.md) for the architecture and [`CLAUDE.md`](CLAUDE.md) for working conventions. **Status: live.** The Discord shim, the LLM ops agent (Gary), and the in-pod process supervisor are all deployed and gate-signed via CI; see [`docs/activation-status.md`](docs/activation-status.md) for what's verified.

A Cargo workspace with four crates: `crates/bot` (Discord shim + ops agent + Agones client, binary `grizzly-gameservers`), `crates/supervisor` (in-pod process supervisor baked into game images, binary `grizzly-supervisor`), `crates/control-api` (the wire contract shared by the two — request/response bodies plus the route-path constants and `CONTROL_PORT`, so neither side hardcodes a bare literal like `9359` or `/fs/read`), and `crates/prompt-lib` (build-time compiler that turns the bot's `prompts/` Markdown tree into typed Rust accessors).

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
