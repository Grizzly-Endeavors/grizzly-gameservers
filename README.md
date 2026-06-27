# grizzly-gameservers

A Discord-driven service for spinning up and managing game servers on Grizzly Endeavors hardware. Friends issue commands in Discord; servers run as containers in the homelab Kubernetes cluster (via Agones) and are exposed through the Hetzner proxy VPS. An LLM ops agent handles per-game config tweaks and break-fixing so day-to-day operation doesn't need a human.

See [`docs/design/00-overview.md`](docs/design/00-overview.md) for the architecture and [`CLAUDE.md`](CLAUDE.md) for working conventions. **Status: scaffold — design + structure only, no implementation yet.**

## Build

```bash
cargo build
cargo build --release
```

## Run

```bash
cargo run
```

## Test

```bash
cargo test --quiet
```

## Dev Tasks

Via `just` (convenience, not required):

- `just` / `just ci-local` — fmt-check, lint, test, deny
- `just test` — `cargo test --quiet`
- `just fmt` — `cargo fmt --all`
- `just lint` — `cargo clippy --all-targets -- -D warnings`
- `just deny` — `cargo deny check`

## Git Hooks

Pre-commit and commit-msg hooks live under `.githooks/`. Install with:

```bash
./.githooks/install.sh
```

They enforce formatting, clippy, tests, and conventional commit message format. Bypass is not supported.
