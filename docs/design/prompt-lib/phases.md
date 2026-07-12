# Prompt Library — Implementation Phases

> Module level only. No file or line references — those get mapped in each phase's own session. Each phase is self-contained, depends only on phases before it, and is verifiable on its own. Read `design.md` (same directory) first; it is the design of record and defines every term and contract used here.

**Delivery model:** the entire effort executes on one dedicated feature branch (`feat/prompt-lib`), with per-phase commits landing on that branch. It merges to `main` once, at completion, producing a single CI build/gate-sign/deploy cycle. Per-phase verification is local — build, lints, tests — never a deploy.

## Phase 1 — prompt-lib crate: parse and validate

- **Modules:** a new `prompt-lib` workspace member: the file-format parser (YAML frontmatter + verbatim body), the validated in-memory model of a prompt tree, and the complete build-time validation rule set from the design.
- **Preconditions:** none.
- **Shape when done:** the workspace has the new member, compiling clean under the workspace lints. A function takes a prompt-directory path and returns either the validated model or an error naming the file and the violated rule. Every build-time rule in the design's validation model is implemented: id grammar, uniqueness, and filename match; type discrimination and the schema/`params_from` rules per type; placeholder↔`variables` coverage in both directions; placeholder-name legality; static `tool`/`params` files (no placeholders); the closed `tool_schema` type vocabulary and enum-value grammar; the all-explicit wire-name collision policy; derived Rust item-name collision detection (synthesized params structs, generated enums); unreferenced params files; annotation presence and recursive non-emptiness per type; body leading/trailing-whitespace rejection.
- **Verification:** unit tests (repo sibling-tests layout) cover each validation rule with both an accepting and a rejecting fixture, plus a fixture tree exercising all three file types together. `cargo test --quiet` and clippy pass.

## Phase 2 — codegen emission and build-script entry

- **Modules:** the crate's codegen face: the emitter producing the generated module (prompt structs with infallible `render`; tool unit structs with `NAME` and `spec()`; inline and shared params structs; generated enums; `Option<T>` + `#[serde(default)]` optional fields; the zero-parameter case), the build-script entry function (fixed `prompts.rs` output name in the build output directory, rerun-on-change registration, `Result`-based error reporting), and the cargo feature gating that separates the three faces' dependency graphs.
- **Preconditions:** Phase 1 (the validated model is the emitter's input).
- **Shape when done:** given a valid prompt tree, the entry function emits `prompts.rs` implementing the design's full generated-code contract, and the emitted code satisfies the workspace lint regime (via codegen-emitted scoped expectations where needed). The runtime face exposes the `ToolSpec` type and nothing heavier; codegen-only dependencies (the YAML parser) sit behind the codegen feature.
- **Verification:** prompt-lib's own tests compile and exercise emitted code from fixture trees (mechanism is the phase session's choice — e.g. a fixture tree consumed by the crate's own build script, or a compile-test harness). Rendered prompt output is byte-compared against expected strings; assembled schemas are JSON-compared covering `required`, `additionalProperties`, enum values, optional fields, and the zero-parameter shape; `NAME` derivation and struct sharing via `params_from` are asserted. `cargo test --quiet` and clippy pass, including over generated output.

## Phase 3 — annotation verification and agent contract

- **Modules:** the crate's verification face (`verify_annotations`), the conventional-test pattern for consuming crates, and the repo's CLAUDE.md (the agent maintenance contract section).
- **Preconditions:** Phase 1 (parsing and model; codegen is not required).
- **Shape when done:** `verify_annotations(prompts_dir, src_dir)` implements the design's test-time checks — every `used_by` entry names an existing source file whose text contains both the id and the named function, and no prompt id is absent from the entire source tree — with failures naming the prompt file and the stale entry. The agent maintenance contract is written into the repo's CLAUDE.md, including the conventional-test snippet and the YAML file conventions.
- **Verification:** unit tests with fixture prompt trees and fixture source directories cover each failure class (missing file, file lacking the id, file lacking the function, orphaned prompt) and the passing case. `cargo test --quiet` passes.

## Phase 4 — bot adoption and conversation-text migration

- **Modules:** the bot crate's build wiring (build script calling the codegen face, the `prompts/` tree, the generated-module include, the conventional annotations test) and the migration of all conversation text: the tier-gated Discord system-prompt blocks, the in-game system prompt, the deferred-task batch prompt and its trigger-note fragments, the in-game question framing, and the fixed framing prose around rendered memories.
- **Preconditions:** Phases 1–3.
- **Shape when done:** those texts live as `type: prompt` files with complete annotations and render through generated accessors; the assembly functions keep their shape, selecting and concatenating blocks with code-owned separators; the assembled bytes sent to the model are identical to pre-migration output; the annotations test is wired and green.
- **Verification:** the existing behavior tests asserting on substrings of assembled prompts pass unchanged; the full bot test suite and the new annotations test pass; assembled-prompt output is byte-compared against pre-migration output for each tier and surface (temporary comparison tests or a recorded manual diff — the phase session's choice, removed once confirmed).

## Phase 5 — tool migration

- **Modules:** both surfaces' tool-definition modules and dispatchers; the shared `type: params` files; generated enums and the explicit domain-narrowing conversions at the dispatch boundary; the one-place `ToolSpec`-to-wire-type conversion glue; removal of the schemars dependency and the schema-stripping helper.
- **Preconditions:** Phases 1–4 (Phase 4 supplies the bot's build wiring).
- **Shape when done:** every tool on both surfaces is defined by a `type: tool` file — the in-game variants via distinct ids sharing wire names through the explicit `name` override — with shared parameter shapes expressed as params files preserving today's deliberate struct sharing. Dispatch matches on generated `NAME` constants and deserializes into generated params structs; fallible narrowing feeds explanatory error strings back to the model as tool results. The schemars dependency is gone.
- **Verification:** the full test suite passes, including updated schema-shape tests; assembled schemas are semantically equivalent to the pre-migration ones modulo the documented `additionalProperties` tightening; no hand-written tool name or description literal remains in the tool-definition modules; the annotations test passes.

## Phase 6 — tool-result prose and final sweep

- **Modules:** every dispatch and result-formatting path in the bot that sends fixed prose back to the model (both surfaces' tool executors, the deferred-task watcher, backup/archive/restore result copy), plus a final scope-rule sweep of the whole bot crate.
- **Preconditions:** Phases 1–5.
- **Shape when done:** the fixed model-facing phrases in tool results, refusals, and error copy render from prompt files; per-item data formatting and joining stay in code and enter through variables; value-level fallbacks are documented in the owning variable's annotations; no model-visible prose remains as a string literal in bot source; every prompt file's annotations are complete and current.
- **Verification:** the full suite and the annotations test pass; a documented audit sweep of the bot source (grep for model-bound string construction across dispatch, session, and formatting paths) confirms the done-criterion; the cumulative branch diff is reviewed against the text-preservation invariant.

## On completion (not a phase)

Merge the feature branch to `main` and watch the single CI deploy. End-to-end verification then checks the assembled whole against the design's intent: Gary behaves identically on a Discord conversation, a tool-calling operation, an in-game question, and a deferred `run_when` task; and a deliberate prompt-file violation (e.g. an undeclared placeholder) fails the local build with the file and rule named.
