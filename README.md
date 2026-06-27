# grizzly-gameservers

A Rust project.

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
