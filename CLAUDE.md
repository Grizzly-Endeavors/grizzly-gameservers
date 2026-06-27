# CLAUDE.md | grizzly-gameservers

This file provides guidance to Claude Code when working with code in this repository.

## Project Overview

A Discord-driven service that lets non-technical friends spin up and manage game servers on Grizzly Endeavors hardware. Game servers run as containers in the homelab Kubernetes cluster (via **Agones**); the Hetzner proxy VPS is the public edge. A friend issues a command in Discord and gets back a server address; when something needs tuning or breaks, they ping the bot. The audience is non-technical, so the bot owns the whole experience.

**Read `docs/design/00-overview.md` first** — it's the architecture of record. The short version:

- **Discord shim** — thin: auth the friend, map a command to an Agones allocation/teardown, return `IP:port`. No quotas/billing (friends-scale).
- **Ops agent** — the center of gravity. An LLM loop that operates running servers (read config+logs via Agones/k8s exec, mutate the right file, restart, verify, roll back). It exists to absorb each game's idiosyncratic config layout *generically* instead of hand-rolling a per-game adapter for every game. Hard guardrails: namespace-scoped blast radius, snapshot→apply→verify→auto-rollback, and an escalation exit.
- **Agones** — underneath, for lifecycle, dynamic port allocation, health, and the exec substrate. Not hand-rolled.
- **Two config tiers** — *per-game* (catalog in `games/`, version-controlled, gated, Flux-deployed) vs. *per-instance* (on the server's PVC, mutated live by the agent, never in git).

This is a gated first-party app under the ADR-020 delivery model: root `gate-config.json` honest map, gate-signed images, Flux renders `deploy/`. Agones standup and the agent's guardrails live in `cluster/` **here** (so the gate vets them), not in `grizzly-platform`. `grizzly-platform` keeps only the Flux registration and the edge port-range forwarding (**7000–7010**, UDP+TCP, over the `wg0` tunnel).

### Repo layout

- `crates/` — Rust workspace. `bot/` (Discord shim + ops agent + Agones client + config loader, binary `grizzly-gameservers`), `supervisor/` (in-pod process supervisor baked into game images, binary `grizzly-supervisor`), `control-api/` (wire types shared by the bot's control client and the supervisor's control server). Strict lints live in `[workspace.lints]` at the root `Cargo.toml`; each crate opts in with `[lints] workspace = true`.
- `deploy/` — Helm chart Flux renders (the bot/agent workload).
- `cluster/` — Agones standup, agent guardrails (namespace/RBAC/NetworkPolicy), Kyverno image carve-out.
- `games/` — per-game base GameServer/Fleet templates.
- `docs/design/` — design of record; `docs/decisions/` — ADRs as decisions resolve.

## Running the bot locally

Use `scripts/local-bot.sh {start|stop|restart|status|logs}` (or `just bot-start` / `bot-stop` / `bot-logs`) to run the Discord bot against the cluster in your current kubeconfig context. Secrets come from the repo-root `.env` (loaded by the binary via dotenvy): `DISCORD_BOT_TOKEN`, `DISCORD_GUILD_ID`, and `GAMESERVERS_ADMIN_USER_IDS` (comma-separated Discord user ids) / `GAMESERVERS_ADMIN_ROLE_ID` to authorize the mutating commands. The script overrides `GAMESERVERS_CATALOG_DIR` to the in-repo `games/` because the compiled default is the in-container path. It builds, launches the binary detached, writes the pid to `target/local-bot.pid`, and streams output to `target/local-bot.log`. After editing `.env` you must `restart` for changes to take effect.

The bot runs locally for dev (no image needed), but the **per-game supervisor runs in-pod**, so iterating on it means building and pushing the composite game image. Use `scripts/push-game-image.sh [game] [tag]` (or `just game-push minecraft dev`): it builds `games/<game>/Dockerfile` (cargo-chef-cached, so a source-only change rebuilds in seconds), port-forwards the in-cluster registry (`registry.registry.svc.cluster.local:5000`, plain HTTP), and pushes through `localhost:5000` (Docker treats localhost as insecure). The `:dev` catalog tag pins `imagePullPolicy: Always`, so a freshly created (or cold-started) server re-pulls. This is the dev loop; the durable path is the gated CI build (see `gh issue` for the build pipeline). Don't full-send every change through CI — the bot iterates locally in seconds, and only the supervisor needs an image.

## Naming

- **Domain-specific names**: prefer descriptive names that match the domain (`send_chat_completion` over generic `run`, `spawn_widget_window` over `handle`).
- **Common abbreviations OK**: `cfg`, `dir`, `msg`, `ctx`, `cmd` are fine; avoid obscure ones.
- **Semantics must match logic**: structs, enums, and functions should make it abundantly clear what they do. Avoid vague names and catch-alls. If the logic doesn't match the semantics of the name, refactor the logic or rename it — don't let the name lie.

## Error Messages

- **Always include context**: `"failed to parse config at {path}"` — not just `"parse error"`.
- **Lowercase, no trailing period** — Unix style, chains cleanly with context wrappers (`anyhow::Context`, `errors.Wrap`, `Error.cause`, etc.).
- **User-facing vs developer-facing**: developers get structured, chained context; end users get plain-language, actionable messages. These are different audiences — don't conflate them.

## Comments

- **Explain *why*, never *what***. The code shows what; comments exist for non-obvious reasoning — hidden constraints, subtle invariants, workarounds for specific bugs, behavior that would surprise a reader.
- **No comments that restate the code**: `// increment counter` on `counter += 1` is noise. Delete it.
- **Doc comments**: one-line summary for public items. Expand only when behavior is non-obvious (error modes, performance notes, thread-safety). Don't narrate parameters that are already named.
- **No planning / decision / analysis comments in shipped code**. Those belong in docs, commit messages, and PR descriptions, not in the file.

## Visibility

- **Private-first**: start with no visibility modifier (or the language's equivalent: unexported, module-private, file-scoped). Add `pub` / `public` / `export` only when there's a consumer that demands it.
- **Treat public as a commitment**: once something is public, it's API. Future changes cost consumers. The cheapest API change is the one you never made public in the first place.
- Prefer package-internal visibility (`pub(crate)`, internal module re-exports) over fully public when the consumer is inside the same project.

## Testing

**Testing is a first-class operation — NEVER skip test implementation.**

- Every pure module ships with tests on the day it lands. "I'll add tests later" is how untested code accumulates.
- **Unit tests** live next to or inside the code they test (language-dependent placement: inline `#[cfg(test)]` module in Rust, `_test.go` sibling in Go, `test_*.py` sibling in Python, etc.).
- **Integration tests** live in a dedicated directory (`tests/`, `integration/`, or language idiomatic equivalent).
- **Always run tests with a quiet flag** where available (`cargo test --quiet`, `pytest -q`, etc.) — suppresses per-test noise, surfaces failures and summaries.
- The shell (IO, framework, GUI) is usually not unit-tested; it's exercised via integration tests or manual verification. But the *logic* reachable from the public API should be testable without the shell.
- Don't test implementation details — test behavior. A refactor that preserves behavior shouldn't break tests.

## Observability — No Silent Failures

**Every failure must be visible.** This is non-negotiable.

### User-facing: clear, actionable messages

- Assume end users are non-technical. Error messages must explain what went wrong and what to do next — not expose internals.
- Plain language: `"Couldn't connect to the server. Check your internet connection and try again."` — not `"TCP connection refused on port 443"`.
- Partial failures (e.g., 2 of 3 items synced) must tell the user what succeeded, what failed, and whether action is needed.
- Never show raw error types, stack traces, or module paths to end users.
- If an operation fails silently with no user impact, it still needs a log.

### Developer-facing: rich, structured diagnostics

- Every error path produces a log entry with enough context to diagnose without reproducing.
- Use structured fields — `error!(error = %e, path = %path, "failed to read config")` — not prose string interpolation.
- Chain error context at each layer so the log shows the full causal chain (`anyhow::Context` in Rust, `fmt.Errorf("...: %w", err)` in Go, exception chaining in Python).
- Pick the right level: `error` (operation failed), `warn` (recoverable/degraded), `info` (major lifecycle event), `debug` (state transitions), `trace` (payloads, per-tick).

### Avoid log spam

- Do not log every retry individually — log once at `warn` when retries start, and once when they resolve or exhaust.
- Do not log routine successful operations ("heartbeat ok", "connection alive"). Absence of errors is the signal that things work.
- Anything that would fire on every frame, poll tick, or event under normal conditions belongs at `trace` at most, never `info`.

## Git Workflow

**Single-branch model**: all work lands on `main`.

### Day-to-day

1. Create a feature branch from `main` with a prefixed name (`feat/`, `fix/`, `refactor/`, `docs/`, `ci/`, `chore/`).
2. **Commit frequently** — especially during large multi-phase tasks. Pre-commit hooks enforce fmt, lint, and tests.
3. Wrapup: check for anything unfinished, update docs/guides, create migration guides for breaking changes.
4. Push the branch and merge into `main`.

### Rules

- **Never use `git -C`** — the shell is already in the project root. Use plain `git` commands.
- **Use `git commit -m "message"` only** — no HEREDOC, no `$()`, no `cat <<EOF`. These alternate formats fail on this author's setup and will be rejected.
- **Hook bypass (`--no-verify`) is FORBIDDEN.** If a hook fails, fix the underlying issue.
- **First-line commit conventions**: ≤72 chars, lowercase, imperative mood, conventional prefix (`feat:`, `fix:`, etc.), no trailing period.
- All changes must be committed before giving the user a completion summary.

## Lint Configuration

Clippy is configured with strict denies — not warnings. The lint block in `Cargo.toml` is intentional and not to be relaxed without explicit approval.

### Why strict

These rules are strict because this project is primarily developed with AI coding agents, and agents will take every shortcut that isn't explicitly denied. Warnings get ignored; only hard errors change behavior. The strictness is a substitute for the discipline a human developer would bring naturally.

### What's denied and why

- **`unwrap_used`, `expect_used`, `panic`, `get_unwrap`**: no panics on untrusted input or error paths. Use `?`, `anyhow::Context`, or explicit handling.
- **`todo`, `unimplemented`, `dbg_macro`**: no incomplete or debug code ships.
- **`exit`**: only `main.rs` may call `std::process::exit`. Library code returns `Result`.
- **`indexing_slicing`, `string_slice`**: use `.get()` / slice-returning methods that produce `Option`.
- **`map_err_ignore`, `let_underscore_must_use`**: no silent error swallowing.
- **`missing_errors_doc`, `missing_panics_doc`, `must_use_candidate`**: public API must document its failure modes and must-use returns.
- **`allow_attributes`, `allow_attributes_without_reason`**: every suppression uses `#[expect(..., reason = "...")]`, never `#[allow(...)]`. `#[expect]` warns when the suppression goes stale.
- **`tests_outside_test_module`**: tests live in `#[cfg(test)] mod tests` blocks (or in `tests/` at crate root for integration — those files need the lint-level escape at the file top).
- **`wildcard_enum_match_arm`, `shadow_unrelated`**: no lazy match catch-alls, no surprise variable shadowing.
- **`rc_buffer`, `rc_mutex`**: antipatterns.
- **`clone_on_ref_ptr`, `format_push_string`, `redundant_type_annotations`**: code quality.

### Relaxed

- `pedantic` is at `warn` with `priority = -1`: pedantic baseline, but deny would break builds on toolchain upgrades that add new lints.
- `module_name_repetitions = "allow"`: pervasive pattern in this codebase style.

### Test modules

Use `#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]` at the top of `#[cfg(test)] mod tests` — not `#[allow]`.

### Unsafe

`unsafe_code = "deny"` (not `forbid`) — `deny` allows scoped `#[expect(unsafe_code, reason = "FFI boundary")]` at the item level when genuinely needed. `forbid` can't be overridden item-level.

## Async

- **Tokio is the default runtime.** `#[tokio::main]` in `main.rs`, `tokio::spawn` for background tasks, `tokio::select!` for concurrent I/O supervision.
- **Async-first**: use async for I/O; fall back to sync only for CPU-bound or genuinely trivial operations.
- **No blocking calls inside async functions.** Use `tokio::fs`, `tokio::process`, etc. If a blocking call is unavoidable, wrap it in `spawn_blocking`.
- **Shared state**: wrap in `Arc<T>` with thread-safe interior mutability (`DashMap`, `Mutex`/`RwLock` when contention is low, channels for message passing).
- **Graceful shutdown**: listen for `SIGINT`/`SIGTERM` via `tokio::signal`; let the `tokio::select!` in the main loop fall out on signal and drain in-flight work.
- **Tracing init happens before anything else async starts** — `EnvFilter::from_default_env()` reads `RUST_LOG` at startup.

## Error Handling

- **`anyhow::Result<T>`** at subsystem boundaries and inside binaries. Chain context with `.context("failed to load user settings")` so logs show the causal chain.
- **`thiserror` enums** for domain errors that callers match against or that need to map to specific outcomes (HTTP status codes, exit codes, user-facing error kinds).
- **`anyhow` wraps `thiserror`**: domain errors bubble up as typed errors; boundaries widen them to `anyhow::Error` with added context.
- **`main.rs` is the only place that maps `Result` to exit code.** Every other function returns `Result`; the `exit` lint is denied elsewhere.
- **Error messages**: lowercase, no trailing period, context-first (`"failed to parse config at {path}"`).
- **No `.unwrap()` / `.expect()` outside of tests** — the lints deny them. If you genuinely know a value is present, use `.expect("reason — invariant explanation")` inside a scoped `#[expect(clippy::expect_used, reason = "...")]` with a real reason.
- **No silent error swallowing.** `let _ = ...` on a `Result` is denied; so is `.map_err(|_| ...)`. Every error either propagates or gets logged with context before being handled.

## Module Layout

- **`mod.rs`** primarily contains declarations and curated `pub use` re-exports. Module-level coordination logic is fine when it belongs there; gratuitous plumbing is not.
- **Group related types in one file** (e.g., `Message`, `Role`, `ToolCall` together in `llm/types.rs`) rather than one-type-per-file.
- **Shell vs core split**: IO-bound / framework-bound code (HTTP handlers, GUI callbacks, Bevy systems, event loops) lives in a thin *shell* layer that calls into *pure* modules where all the logic is. The shell is usually not unit-tested; the pure modules are.
- **Binary vs library**: even binary crates benefit from a thin `main.rs` that delegates to a library module — makes the logic testable and keeps `main.rs` at the "load env → init tracing → dispatch → exit code" skeleton.
- Test placement is covered in its own section (see "Test Organization (Rust)").

## Test Organization (Rust)

Testing is a first-class operation — NEVER skip test implementation. Always run `cargo test --quiet`; never plain `cargo test` (the `--quiet` flag suppresses per-test noise and surfaces failures + summary).

### Unit test file layout

Unit tests do **not** live at the bottom of the impl file. They live in a sibling `tests/` subdirectory, loaded via a `#[path]` module declaration at the bottom of the impl file. This keeps impl files short and prevents a three-line edit from dragging hundreds of lines of test noise into the LLM context window.

Per-module rules:

- `src/foo.rs` → tests at `src/tests/foo.rs`, declared in `src/foo.rs` as:
  ```rust
  #[cfg(test)]
  #[path = "tests/foo.rs"]
  mod tests;
  ```
- `src/foo/bar.rs` → tests at `src/foo/tests/bar.rs`, declared in `src/foo/bar.rs` as:
  ```rust
  #[cfg(test)]
  #[path = "tests/bar.rs"]
  mod tests;
  ```
- `src/foo/mod.rs` → tests at `src/foo/tests/foo.rs` (named after the module, not `mod.rs`), declared in `src/foo/mod.rs` as:
  ```rust
  #[cfg(test)]
  #[path = "tests/foo.rs"]
  mod tests;
  ```

The `#[path]` attribute makes the loaded file a **child** of the impl module, so `use super::*;` retains full access to private items — no visibility inflation.

### Unit test file boilerplate

Every sibling test file starts with:

```rust
#![expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]

use super::*;
// ...
```

### Integration test boilerplate

Every `tests/*.rs` file at crate root must start with:

```rust
#![expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#![expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live at crate root by cargo convention"
)]
```

The second attribute is required because `clippy::tests_outside_test_module` is denied project-wide and fires on top-level `#[test]` functions — which are exactly what integration tests are.

### What gets tested

- **Pure modules** (parsing, validation, state transitions, path manipulation, error construction): every pure module ships with tests on the day it lands.
- **Shell modules** (GUI callbacks, HTTP handlers, framework integrations, event loops): usually not unit-tested. Exercised via integration tests or manual verification.
- **Logic reachable from `lib.rs`**: testable without the shell. If you find yourself adding logic to a shell module, move it into a pure module first and let the shell call the validated result.

### Env var handling in tests

Rust 2024 made `std::env::set_var` / `remove_var` `unsafe`. Do not use them in tests. Instead, structure env-reading functions as a dual pair:

- Public zero-arg function that reads the real process env (e.g., `wayland_session()`).
- Public `_from_env` sibling that takes an `EnvLookup<'_>` closure (e.g., `wayland_session_from_env(get_env)`).

The public function calls the sibling with `&|k| std::env::var_os(k)`. Tests construct closures with fixed keys: `(k == "WAYLAND_DISPLAY").then(|| OsString::from("wayland-0"))`.

### Rationale

This layout is optimized for LLM agent workflows where the default "read the whole file" cost is multiplied by every edit. Sibling test files preserve Rust's native test ergonomics (private access, `cargo test` runs them automatically) while dramatically cutting read-time noise on mature modules.

