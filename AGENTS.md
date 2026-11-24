# AGENTS.md | grizzly-gameservers

Project-specific guidance for agents working in this repository. Universal rules live in `~/Projects/AGENTS.md`.

## Product Invariants

- **Gary is the product; `grizzly-gameservers` is the repo.** Every code identifier uses `grizzly-gameservers`; user-facing prose says Gary. Any change must make Gary a more capable or safer operator (never widen unsupervised blast radius for hosting convenience).
- **Gary is the only reader of a server's PVC.** Friends never see `.grizzly/env`, `.grizzly/mods`, or per-instance files — optimize instance layout for unambiguous agent parsing and self-verification, never an imagined human reader.
- **No `kubectl exec`:** Gary acts through the per-pod supervisor's typed HTTP control API (`crates/control-api/`). Guardrails (namespace blast radius, snapshot→apply→verify→auto-rollback, `escalate` exit, bounded rounds) are the core product.
- **Gated first-party app:** Root `gate-config.json` honest map, gate-signed bot images, Flux renders `deploy/`. Agones standup and RBAC/NetworkPolicy guardrails live in `cluster/` here; `grizzly-platform` keeps only Flux registration and Hetzner edge forwarding (ports **7000–7099** UDP+TCP over `wg0`).

## Deploy, Run & Operate

- **Bot deploys via CI:** Pushing to `main` builds, gate-signs, and pins the digest into `deploy/values.yaml` for Flux. Watch with `gh run list --branch main` / `gh run watch <id> --exit-status`. Never `kubectl rollout restart` the bot (it re-pulls the pinned digest and races Flux).
- **Exercise the deployed bot** (no Discord client needed; see `drive-the-bot` skill):
  - `just operator tool <name> ['<json>']` — run a tool directly (default: deterministic, fast, free).
  - `just operator gary "<prompt>"` — run a full Gary turn (only when testing reasoning).
  - `just operator tools [--access read-only|manager|admin]` — inspect tool schemas (`--confirm` pre-approves admin destructive prompts).
  - `just bot-logs-deployed` — tail live pod logs.
- **Supervisor / game image dev loop:** `just game-push <game> dev` builds `games/<game>/Dockerfile` and pushes to the in-cluster registry via `localhost:5000`. Game images are not CI-built yet.
- **Off-cluster local bot:** `just bot-start` / `bot-stop` / `bot-logs` (reads `.env`; restart after editing `.env`).

## External Services & Operational Traps

- **Settings precedence:** Postgres override > env var > compiled default (`SettingKey` enum in `crate::settings`, read live from `ArcSwap`). Secrets stay in 1Password/ESO, never in settings.
- **Degraded modes:** `DB_PASSWORD` (R730xd Postgres) enables home channels, `/config`, events, occupancy/resource sampling, and watchdog; `REDIS_PASSWORD` (R730xd Valkey DB 2) enables `run_when`; `LANGFUSE_PUBLIC_KEY`/`SECRET_KEY` enable trace export (Langfuse v4 is `events_only` — read `docs/operations/data-collection.md` before querying); `GAMESERVERS_SMTP_PASSWORD` (`gary@grizzly-endeavors.com` Stalwart) enables signup approval mail and **latches off on refused credentials** so bad retries don't get the household IP banned.
- **Allowlist trap:** Empty `GAMESERVERS_GUILD_ALLOWLIST` serves **every** guild. Read `docs/operations/signups.md` before opening public signups, and `docs/operations/admin-panel.md` before changing the admin panel (`https://gameservers-admin.grizzly-endeavors.com`, port 9361, Authentik forward-auth).

## Prompt Library (`crates/bot/prompts/`)

All model-visible prose (system prompts, tool names/descriptions/params, fixed tool-result prose) lives under `crates/bot/prompts/<feature>/<Id>.md` and compiles into typed Rust via `grizzly-agent-prompts`.

- **Load-bearing contract:** Any change that renders a prompt, adds/removes a `{{variable}}`, or moves a call site **must update that prompt file's `annotations` (`used_by`) in the same change** (verified by the integration test `prompt_annotations_are_current`).
- **Human tuning surface:** You may create prompt files, but **do not rewrite existing model-visible prompt prose without being asked**.
- **YAML conventions:** Double-quote values containing `:`, use `>-` for multi-line prose, PascalCase `id` matching filename stem (no acronym runs: `HttpProxy`), `used_by` entries use `{file, function}` relative to `src/` (never line numbers).
- **Same-rev pin:** All three `grizzly-agent` / `grizzly-agent-prompts` git deps in `crates/bot/Cargo.toml` must point to the exact same `rev`.

## Rust Workspace Rules

- **Strict lints:** Defined once in root `[workspace.lints]` and inherited via `[lints] workspace = true`.
- **Test-module lint trap:** `clippy.toml` exempts `unwrap_used`, `expect_used`, `panic`, and `dbg_macro` in tests. **Do not add `#[expect(clippy::unwrap_used, ...)]` to test files** — it becomes an unfulfilled-expectation error under `-D warnings`.
- **Integration tests (`tests/*.rs`):** Start with `#![expect(clippy::tests_outside_test_module, reason = "integration tests live at crate root by cargo convention")]`.
- **Sibling unit tests:** Put unit tests in `tests/<module>.rs` loaded via `#[cfg(test)] #[path = "tests/<module>.rs"] mod tests;`. Run `cargo test --quiet`.
- **Env vars in tests:** Never call `unsafe` `std::env::set_var` / `remove_var`; expose a `from_env(EnvLookup<'_>)` sibling.
