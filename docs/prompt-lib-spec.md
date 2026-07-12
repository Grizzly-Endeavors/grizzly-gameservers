# Prompt Library — Design Specification

Status: superseded by `docs/design/prompt-lib/design.md` (the design of record, with implementation phases in `phases.md` alongside). Kept for history; the design pivoted from runtime file loading to full compile-time embedding, among other changes.

## Problem

LLM-integrated projects accumulate prompts scattered across the codebase — embedded in string literals, hard-wrapped, entangled with tests, and effectively written by AI coding agents rather than a human. The result is software whose LLM behavior is hard to observe and hard to tune: the knobs exist, but they're buried.

This crate externalizes every instruction the software sends to a model — system prompts, tool descriptions, tool parameter descriptions — into plain files with a single convention, so that all model-visible text lives in one place, is editable in any text editor, and carries the context needed to tune it without digging through the codebase.

## Design Principles

1. **The body is the payload.** What you see in the file below the frontmatter is byte-for-byte what the model receives (modulo `{{variable}}` interpolation). No escaping, no wrapping, no indentation management, no syntax burying the content.
2. **One file per prompt.** Structure lives in the directory tree and the frontmatter, not inside a document format. Filenames and `id`s are the schema.
3. **Annotations are for the human, maintained by the agents.** Frontmatter carries the context needed to tune a prompt — where it's used, when it's sent, what its variables mean, why it's designed the way it is. AI agents writing the code are responsible for keeping this current as part of any change.
4. **Enforce what's mechanically verifiable; accept what isn't.** Structural validity, placeholder/annotation coverage, and call-site references are checked by machine. The accuracy of prose is a social contract with the agents, not a technical one.
5. **Prompt tuning must not require recompilation.** Prompt bodies are read from disk at startup, so wording edits are edit → restart. Only structural changes — adding, removing, or renaming a variable — trigger a rebuild, and those already require touching caller code anyway.
6. **Variables are part of the type system.** A build step generates a typed accessor per prompt whose fields are its `{{variables}}`. A wrong or missing variable is a compile error, not a runtime surprise — and not something an agent can break silently.
7. **This crate does not talk to models.** No API clients, no model or sampling config, no `send()`. Its job ends at "here is the exact string and schema." Model configuration lives at the caller level, where it already has established patterns.

## File Format

Prompt files are Markdown with YAML frontmatter. Everything after the closing `---` delimiter is the prompt body, verbatim (a single trailing newline is trimmed). Variables use `{{name}}` placeholder syntax.

### Common frontmatter

**`id`** (string, required) — Durable identifier used by code to reference this prompt. Also the greppable reverse index: searching the codebase for the id yields live call sites.

**`type`** (`prompt` | `tool`, required) — Discriminator. `tool` files must carry `tool_schema`; `prompt` files must not.

**`annotations`** (map, required) — Human-facing context. Never sent to the model, never consumed by application logic. Fields:

- `sent_when` — When this prompt is sent, described at the behavioral level ("user has auto-summary enabled and thread exceeds threshold"), not in code coordinates.
- `used_by` — List of `{file, function}` entries identifying call sites. File + function granularity, never line numbers.
- `variables` — One entry per `{{placeholder}}` in the body, each with `source` (where the value comes from) and `contents` (what it contains).
- `reasoning` — List of design-intent notes: why the prompt exists, why it's shaped the way it is. This is the context that prevents a future editor (human or agent) from "fixing" a prompt in a way that breaks its purpose.

### Tool frontmatter

**`tool_schema`** (map, `type: tool` only) — Parameter definitions from which the loader assembles the JSON schema sent to the model. Each parameter carries `type` and `description`. Parameters are required by default; mark exceptions with `optional: true`.

Parameter descriptions are model-visible prompt text and get the same care as the body. Placement rule for any piece of information: if knowing it would change the model's behavior, it belongs in the description (payload); if it only explains the design, it belongs in `annotations.reasoning`. Some information belongs in both, phrased for each audience.

### Example: prompt file

```markdown
---
id: summarize-thread
type: prompt
annotations:
  sent_when: User has auto-summary enabled and thread exceeds threshold
  used_by:
    - file: src/summarize.rs
      function: handle_thread
  variables:
    thread_messages:
      source: DB
      contents: Full message list, oldest-first
    user_instructions:
      source: user settings
      contents: >-
        User's instructions for summarization, if any. Allows the user
        to customize summary style, length, and focus.
  reasoning:
    - Long threads can exceed the context window, so we summarize to
      keep the conversation manageable.
---
You are summarizing a conversation thread for a busy reader.

{{user_instructions}}

Focus on decisions made and open questions...
```

### Example: tool file

```markdown
---
id: edit-file
type: tool
tool_schema:
  file_path:
    type: string
    description: The path to the file to edit.
  old_content:
    type: string
    description: >-
      Must exactly match the current file content at the target
      location. Used to locate the edit and to reject the write if
      the file has changed since it was read.
  new_content:
    type: string
    description: The new content to write in place of old_content.
annotations:
  sent_when: always
  used_by:
    - file: src/tools.rs
      function: handle_edit_file
  reasoning:
    - old_content doubles as a concurrency guard, preventing
      accidental overwrites when the file changed after reading.
---
Edits a file by replacing an exact existing span with new content...
```

### YAML conventions

Quote values containing colons. Use block scalars (`>-`) for multi-line prose in annotations and descriptions. These conventions are part of the agent contract so files stay uniform.

## Directory Layout

One directory of prompt files per project, organized by feature. The tree is the namespace; files are the leaves.

```
prompts/
  summarize/
    system.md
  tools/
    edit-file.md
    search-notes.md
```

## Library API (high level)

**Generated accessors (build step).** A `build.rs` in the consuming project — a one-line call into a helper exposed by this crate — parses the prompt directory and generates a module of typed accessors: one struct per prompt whose fields are its variables (with a `render` method), and one typed definition per tool. `cargo:rerun-if-changed` on the prompt directory keeps generation current. These accessors are the primary interface; call sites reference prompts through them, never through raw id strings.

**`PromptLibrary::load(path) -> Result<PromptLibrary>`** — Reads every file in the directory eagerly at startup (e.g., top of `main`) and acts as the drift guard between disk and compiled types (see Validation Model). Any problem fails fast with a message naming the file and the violation, before the application does anything else.

**`render(id, vars) -> Result<String>`** — Dynamic, string-keyed rendering; the mechanism beneath the generated accessors, available directly for edge cases. Rejects missing or extra variables with a clear error. The body's placeholders are the authoritative variable list; callers cannot silently drift from it.

**`tool(id) -> Result<ToolDefinition>`** — Assembles the model-facing tool definition (name, JSON schema with per-parameter descriptions, description body) from a `type: tool` file.

**`verify_annotations(prompts_dir, src_dir) -> Result<()>`** — Cross-references annotations against the source tree. Intended for tests only, never for runtime — a shipped binary does not scan its own source.

## Validation Model

Checks run at four moments, matched to the severity of what they catch.

**Build time (primary structural validation).** The codegen step validates everything structural: frontmatter parses; `id` present and unique; `type` valid; `tool_schema` present exactly when `type: tool`; every `{{placeholder}}` in the body has an `annotations.variables` entry and every entry corresponds to a real placeholder; `tool_schema` parameters are well-formed. Violations fail the build with the file and rule named. Variable mismatches at call sites need no dedicated check at all — they are ordinary compile errors against the generated types.

**Load time (startup drift guard).** Because bodies are read from disk at runtime, files can change after the binary was built. `load()` re-runs structural validation and confirms that each file's placeholders still match its compiled accessor; any drift fails fast at startup with a message to rebuild, rather than ever rendering a prompt with an unfilled placeholder. Pure wording edits pass untouched — this guard only trips on structural change.

**Render time.** On the dynamic path, the variables passed by the caller exactly match the body's placeholders — no missing, no extras. (The typed path makes this unrepresentable.)

**Test time (CI, blocks merge but not iteration).** Each project carries one conventional test:

```rust
#[test]
fn prompt_library_is_valid() {
    PromptLibrary::load("prompts/").unwrap();
    prompt_lib::verify_annotations("prompts/", "src/").unwrap();
}
```

Structural validity is already covered by the build, which `cargo test` triggers; this test exists for the checks that need to see the rest of the source tree: every `used_by` entry points at a real file that references the id, and no prompt is orphaned. Stale cross-references fail CI rather than the build, because a stale comment shouldn't stop you from compiling mid-refactor — it should stop you from merging.

**Not enforced.** The truthfulness of `sent_when`, `reasoning`, and variable prose. Verifying prose accuracy by machine is a losing game; it's covered by the agent maintenance contract and by the fact that stale prose is noticed at exactly the moment it matters — when someone is in the file tuning.

## Agent Maintenance Contract

This convention ships with the crate (as a skill / CLAUDE.md snippet) and is half the system:

Any change to code that renders a prompt, adds or removes a prompt variable, or moves a call site must update the corresponding prompt file's `annotations` in the same change. `used_by` and variable coverage are enforced by the test; `sent_when` and `reasoning` are maintained on trust. Prompt bodies and tool/parameter descriptions are the human's tuning surface: agents may create them but should not rewrite existing model-visible text without being asked. New prompts follow the YAML conventions above.

## Non-Goals

Model and sampling configuration; API clients or transport; provider abstractions; a standalone lint binary (subsumed by build/load-time validation and the conventional test); prompt versioning, A/B testing, or analytics.

## Future Extensions (explicitly deferred)

**Embedded prompts.** An opt-in feature flag using `include_dir!` for self-contained deployment binaries, at the cost of the no-recompile edit loop. The default remains files-on-disk.

**Publication.** Extract from the workspace and publish once the format survives real use in the host project.