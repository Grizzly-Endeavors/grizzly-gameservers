# Prompt Library — Design

> Systems level only. This document is the design of record for the `prompt-lib` workspace crate and the migration of the bot's model-visible text into it. It supersedes the draft spec at `docs/prompt-lib-spec.md`. Implementation happens in fresh sessions that have only this document, `phases.md`, and the codebase — it must stand on its own.

## Goal & context

LLM-integrated projects accumulate prompts scattered across the codebase — embedded in string literals, entangled with the code that assembles them, and effectively written by AI coding agents rather than a human. The result is software whose LLM behavior is hard to observe and hard to tune: the knobs exist, but they're buried.

`prompt-lib` externalizes every piece of prose this software sends to a model into plain Markdown files with a single convention, so that all model-visible text lives in one place, is editable in any text editor, and carries the context needed to tune it without digging through the codebase. A build step compiles those files into typed Rust accessors, so a wrong or missing variable is a compile error — not a runtime surprise, and not something an agent can break silently.

**The scope rule (durable):** everything the software *sends* to the model is externalized — system prompts, user-turn framing and templates, tool names, tool descriptions, tool parameter descriptions, and the fixed prose of tool *results* fed back into the conversation (tool results are the linchpin for what the model decides next; their wording is among the highest-signal tuning knobs). Out of scope: what the software does with model *output*, text shown to human users, and pure data serialization — computed values, lists, and identifiers enter prompt text through variables, they are not themselves prompt text. A variable's value is either such data or the rendered output of *another prompt file* (see Composition); prose never hides in code either way.

One boundary case gets its own rule because it recurs: **short value-level fallbacks** — a word or phrase standing in for absent data, like rendering a missing game as `unknown game` or an empty list as `(none)`. These are part of computing the variable's *value* and stay in Rust, but they must be documented in the variable's `contents` annotation (e.g. "comma-separated game ids, or `(none)` when the catalog is empty") so the tuning surface still shows them. Sentence-level, instruction-bearing prose is always a file, no matter how conditional its inclusion.

The sole current consumer is the `bot` crate (Gary, the ops agent), which sends model-visible text on two surfaces (Discord and in-game chat) plus a deferred-task batch path. The other workspace crates send nothing to models. The crate incubates in this workspace; extraction and publication are deferred until the format survives real use here.

## Shape

### Components

**The `prompt-lib` crate** — a new workspace member with three faces, dependency-isolated behind cargo features so each consumer edge pulls in only what it uses:

- **Codegen** (feature `codegen`, consumed as a *build-dependency*): one function called from the consuming crate's build script. It walks the crate's prompt directory, runs all structural validation, and emits a single file named `prompts.rs` into the build output directory; the consumer includes it once via `include!(concat!(env!("OUT_DIR"), "/prompts.rs"))` at a module path of its choosing. Codegen registers the prompt directory for rebuild-on-change, so a prompt edit is picked up by the next `cargo build`.
- **Runtime support** (default features, consumed as a *normal dependency*): only the minimal shared types the generated code references — concretely `ToolSpec { name: &'static str, description: &'static str, parameters: serde_json::Value }` with public fields (name and description are embedded statics; the schema value is assembled by each `spec()` call). No YAML parser, no loader, no template engine, no file I/O, no model clients in this face's dependency graph. Prompt bodies exist at runtime only as compiled-in string data inside the generated module.
- **Annotation verification** (feature `verify`, consumed as a *dev-dependency*): `verify_annotations(prompts_dir, src_dir)`, run from one conventional test per consuming crate. It cross-references prompt annotations against the source tree — checks that need to see the rest of the codebase, which the build step deliberately does not.

**The prompt tree** — one `prompts/` directory at the root of each consuming crate. The directory tree is organizational only: subdirectories group prompts by feature, but ids are global and flat. Each file is named exactly `<Id>.md` where `<Id>` is its id.

**The generated module** — included once by the consumer, containing the items described under "Generated code contract" below.

### File format

Prompt files are Markdown with YAML frontmatter. Everything after the closing `---` delimiter is the prompt body, verbatim, except that a single trailing newline is trimmed. Bodies must not begin or end with additional whitespace (build-enforced): when consumer code concatenates multiple rendered blocks into one prompt, the *consumer* owns the separators (e.g. `\n\n` glue), so block files never carry leading or trailing join whitespace. Variables use `{{name}}` placeholder syntax.

**Ids and case conversion.** An id is PascalCase matching the grammar `([A-Z][a-z0-9]*)+` — each segment a capital letter followed by lowercase letters or digits, no acronym runs (`HttpProxy`, never `HTTPProxy`). This makes case conversion trivially unambiguous in both directions: PascalCase → snake_case lowercases each segment and joins with `_`; snake_case → PascalCase capitalizes each segment. Ids are globally unique across the crate's prompt tree, are valid Rust type identifiers, and equal the file's name (stem). Because the id *is* the generated type name, it is also the greppable reverse index: one search for the id finds the prompt file, every call site, and (for tools) the params struct. No id-to-identifier transform exists to drift.

**Common frontmatter:**

- `id` (string, required) — as above.
- `type` (`prompt` | `tool` | `params`, required) — discriminator. `tool` files carry a schema (inline or by reference); `params` files carry `tool_schema` and no body's-worth of behavior (see below); `prompt` files carry neither.
- `annotations` (map, required) — human-facing context, never sent to the model, never consumed by application logic. All present fields must be non-empty (build-enforced), and non-emptiness is recursive: a blank `sent_when`, an empty `used_by` list, or empty `reasoning` fails the build, and so does a blank string sub-field inside any entry (a `used_by` entry's `file` or `function`, a `variables` entry's `source` or `contents`, an empty `reasoning` note).
  - `sent_when` — when this text reaches the model, described behaviorally ("system prompt block appended for managers and admins"), not in code coordinates. For tools: when the tool is offered. Required on `prompt` and `tool` files; not permitted on `params` files (derivable from the referencing tools).
  - `used_by` — list of `{file, function}` entries identifying call sites, at file + function granularity, never line numbers. Required on all three types (for `params` files: the dispatch sites deserializing the shared struct).
  - `variables` — one entry per unique `{{placeholder}}` in the body, each with `source` (where the value comes from — a data origin, or the prompt file(s) whose rendered output feeds it) and `contents` (what it contains, including any value-level fallback under the scope rule's boundary case). Required exactly when the body has placeholders; `tool` and `params` files never have this key.
  - `reasoning` — list of design-intent notes: why the prompt exists and why it's shaped the way it is, so a future editor (human or agent) doesn't "fix" a prompt in a way that breaks its purpose. Required on all three types.

**Tool frontmatter:**

- `name` (string, optional) — the wire name sent to the model. Defaults to the id converted to snake_case. The override exists for surface variants: two files with distinct ids may deliberately advertise the same wire name with different description text (e.g. the terser in-game variants of Discord tools). A wire-name collision is a build error unless *every* colliding file declares the name explicitly — intent must be marked on all sides, so a derived name accidentally matching another file's override still fails the build.
- Exactly one of `tool_schema` (inline parameter definitions) or `params_from` (the id of a `type: params` file) — required. `params_from` must name an existing params file in the tree.

**Params files (`type: params`)** exist because many tools deliberately share one parameter shape — today a dozen server-lifecycle tools all take exactly `{name}`, and the sharing is what stops their contracts drifting apart. A params file carries a `tool_schema` map and generates a single shared params struct named exactly by its id (so a params file's id conventionally ends in `Params`, e.g. `NameParams`, matching what dispatch code declares today). Tool files reference it with `params_from`; every referencing tool advertises that same schema, and dispatch deserializes into the one shared struct. A params file referenced by zero tools is a build error. Parameter *descriptions* in a shared params file are model-visible in every referencing tool — by design: one place to tune them, no copies to drift. v1 has no extension/merging — a tool either uses a params file verbatim or defines its own inline schema.

**`tool_schema`** (inline or in a params file) — parameter definitions, each with `type` and `description`, required by default, `optional: true` for exceptions. The type vocabulary is closed: `string`, `integer`, `number`, `boolean`, `enum`. `enum` parameters carry a `values` list of snake_case strings (grammar `[a-z][a-z0-9_]*`), enforced in the emitted JSON schema and surfaced as a generated Rust enum. Arrays and nested objects are not supported in v1; a parameter declaring them fails the build with an error naming this as a deliberate limit. `tool_schema: {}` is valid — a zero-parameter tool — and generates an empty-object schema and no params struct.

Tool bodies and parameter descriptions are static text: `{{` anywhere in a `tool` or `params` file is a build error. Parameter descriptions are model-visible prompt text and get the same care as bodies. Placement rule for any piece of information: if knowing it would change the model's behavior, it belongs in the description (payload); if it only explains the design, it belongs in `annotations.reasoning`; some information belongs in both, phrased for each audience.

**Variables:** placeholder names match `[a-z][a-z0-9_]*` and must be legal, non-keyword Rust identifiers (they become struct fields). The same placeholder may appear multiple times in a body; `annotations.variables` is keyed per unique name. A literal `{{` in a prompt body has no escape mechanism and fails the build — none of the migrated text needs one, and an escape syntax can be added later if a prompt ever does.

**Composition:** iteration and conditional logic live in Rust, never in the file format. A prompt assembled from tier-gated or otherwise conditional sections is multiple prompt files — one per contiguous fixed block — selected and concatenated by consumer code (which owns the separators), with each file's `sent_when` carrying its condition. Loops (numbered task lists, bullet lists of facts) are joined in code and enter a template through a variable; the unit of externalization is contiguous fixed prose. Formatting that carries no instruction-bearing prose (a `- #3: fact` bullet line) is data serialization and stays in code. Selection among alternatives composes through variables: when code picks one of N complete alternative texts (e.g. a trigger-specific sentence spliced into a larger template's slot), each alternative is its own prompt file and the chosen file's rendered output becomes the value of the outer template's placeholder — the outer variable's `source` annotation names the set of prompts that can feed it, and each alternative's `sent_when` carries its selection condition.

**YAML conventions (part of the agent contract, so files stay uniform):** double-quote values containing colons; use block scalars (`>-`) for multi-line prose in annotations and descriptions.

### Generated code contract

Codegen emits one flat module. Per file:

- **`type: prompt`** → one struct named by the id, one borrowed string field per unique `{{variable}}`, and an infallible `render() -> String`. A prompt with variables renders by consuming `self` (`render(self)`); a prompt with no variables has no `self` to consume, so its `render` is an associated function (`render()`, called `<Id>::render()`) — the borrowless struct is a bare unit struct. The body is compiled into the render method as literal segments interleaved with field values — no runtime templating engine, no error path (validation already happened at build time). Values are inserted verbatim; there is no escaping layer.
- **`type: tool`** → one unit struct named by the id with two associated items: `NAME: &'static str`, the wire name as a constant usable in dispatch `match` arms, and `spec() -> ToolSpec`, assembling the full tool spec (wire name, description body, JSON schema). If the tool has an inline `tool_schema` with at least one parameter, a `Deserialize`-deriving params struct named `<Id>Params` is emitted alongside; if it uses `params_from`, it shares the referenced params file's struct instead.
- **`type: params`** → one `Deserialize`-deriving struct named exactly by the id, shared by every referencing tool.

Sketch of the generated API surface (illustrative, not a file listing):

```rust
// from prompts/gary/InGameSystemPrompt.md, type: prompt, body containing {{games}}
pub struct InGameSystemPrompt<'a> { pub games: &'a str }
impl InGameSystemPrompt<'_> { pub fn render(self) -> String { /* embedded body */ } }

// from prompts/tools/EditFile.md, type: tool, inline tool_schema
pub struct EditFile;
impl EditFile {
    pub const NAME: &'static str = "edit_file";
    pub fn spec() -> ToolSpec { /* assembled at build time */ }
}
pub struct EditFileParams { pub name: String, pub path: String, /* … */ }

// from prompts/tools/NameParams.md, type: params — shared by a dozen lifecycle tools
pub struct NameParams { pub name: String }
```

**Params struct field mapping:** `string` → `String`, `integer` → `i64`, `number` → `f64`, `boolean` → `bool`, `enum` → the generated enum, and `optional: true` → `Option<T>` carrying `#[serde(default)]` — the attribute is load-bearing: without it serde still errors on a missing key, and the model omitting an optional argument must deserialize as `None`, matching today's behavior. Narrowing to domain types (smaller or unsigned ints, hand-written domain enums) happens in consumer dispatch code via explicit fallible conversions; on failure, dispatch feeds an explanatory error string back to the model as the tool result — the existing convention for unparseable arguments — never a panic.

**Generated enums:** an `enum`-typed parameter generates a Rust enum named `<owning file's Id><ParamName in PascalCase>` (e.g. a `condition` parameter in `RunWhen` → `RunWhenCondition`), with one variant per value, PascalCase-converted, serde-renamed to the exact wire string. The rule is unchanged for params files — the owning file is the params file (a hypothetical `mode` enum in `NameParams` → `NameParamsMode`).

**Lint compliance:** generated code must be clean under the workspace's deny-level clippy set (no unwrap/panic/indexing, documented public items, `#[must_use]` where flagged) or carry scoped `#[expect(..., reason)]` emitted by codegen — never hand-maintained.

### Assembled tool schema

The JSON schema for a tool is assembled at build time into `spec()`: an `object` schema whose `properties` carry each parameter's type and description (`enum` parameters emit `"type": "string"` plus an `enum` values array), whose `required` array lists every non-optional parameter (omitted when empty), and with `additionalProperties: false`. The schema is generated clean for OpenAI-compatible endpoints: no `$schema`, no `title` (the current code strips these from schemars output after the fact; generating them correctly makes the stripping obsolete). Relative to today's schemars-derived schemas the assembled ones are semantically equivalent, with `additionalProperties: false` as a deliberate tightening.

### Validation model

Checks run at three moments, matched to what they catch. There is no load-time moment: bodies and accessors are produced by the same build, so disk-vs-binary drift is unrepresentable.

**Build time (all structural validation).** The codegen step validates everything mechanical: frontmatter parses; `id` present, matching the id grammar, unique, and matching the filename; `type` valid; the schema/`params_from` rules for each type as specified above; every `{{placeholder}}` has a `variables` entry and every entry corresponds to a real placeholder; placeholder names are legal identifiers; `tool`/`params` files contain no placeholders; `tool_schema` parameters are well-formed with types from the closed vocabulary; enum values match their grammar; wire-name collisions follow the all-explicit policy; derived Rust item names — the synthesized `<Id>Params` structs and generated enums — don't collide with any declared id or other derived name (reported like wire-name collisions, not left to an opaque compiler error); params files aren't unreferenced; annotation fields present per type and non-empty recursively; bodies carry no leading/trailing whitespace beyond the trimmed trailing newline. Violations fail the build with the file and rule named. Call-site mismatches need no dedicated check — they are ordinary compile errors against the generated types.

**Test time (CI; blocks merge, not iteration).** Each consuming crate carries one conventional test invoking `verify_annotations(prompts_dir, src_dir)`. It checks, textually (no AST parsing): every `used_by` entry names a file that exists under the source tree and whose text contains both the id and the named function; and no prompt is orphaned, meaning every id appears in at least one source file regardless of what `used_by` claims. Failures name the prompt file and the stale entry. This lives at test time rather than build time because a stale cross-reference shouldn't stop compilation mid-refactor — it should stop a merge.

**Not enforced.** The truthfulness of `sent_when`, `reasoning`, and variable prose. Verifying prose accuracy by machine is a losing game; it's covered by the agent maintenance contract and by the fact that stale prose is noticed at exactly the moment it matters — when someone is in the file tuning.

### Agent maintenance contract

This convention ships with the crate as a CLAUDE.md section and is half the system: any change to code that renders a prompt, adds or removes a prompt variable, or moves a call site must update the corresponding prompt file's `annotations` in the same change. `used_by` and variable coverage are enforced by the test; `sent_when` and `reasoning` are maintained on trust. Prompt bodies and tool/parameter descriptions are the human's tuning surface: agents may create them but must not rewrite existing model-visible text without being asked. New prompts follow the YAML conventions above.

## Reasoning & alternatives

**Compile-time embedding over runtime file loading.** The draft spec had prompt bodies read from disk at startup so wording edits wouldn't require recompilation, with a load-time drift guard reconciling disk against compiled accessors. This was dropped. The runtime setup fought the compile-time verification that is the design's actual spine: it required a drift-guard handshake between generated code and a library loader (connective tissue with real design surface), reintroduced runtime failure paths for something knowable at build time, and its headline benefit was illusory for the real deployment — the bot ships as a gate-signed container image reconciled by Flux, so a production wording edit costs a full CI rebuild and redeploy regardless of whether the binary technically recompiled. Embedding makes render infallible, eliminates startup validation and its wiring entirely, ships zero files in the image, and keeps the honest benefits: prompt text is *data, not code* — editable without touching Rust, reviewable as prose diffs, tunable locally with an ordinary incremental rebuild (the build script re-runs on prompt-file change). Files-read-at-runtime is now the deferred extension rather than the default, should live tuning ever matter.

**PascalCase ids over kebab-case plus a transform.** With kebab-case ids, codegen needs an id→identifier transform rule, a collision policy for ids that normalize identically, and the "grep the id to find call sites" principle breaks — call sites contain only the transformed name. Making the id *be* the Rust type name deletes the entire mapping contract and restores greppability exactly. The cost is unconventional-looking filenames (`EditFile.md`); accepted.

**Composition in code over template conditionals.** The bot's Discord system prompt is assembled from ~a dozen blocks gated by access tier. Expressing that in the file format means inventing conditional syntax — a worse Handlebars — and violates "the body is the payload." Per-block files selected by Rust keep logic testable in Rust and text editable as text; `sent_when` documents each block's condition where a human tunes it. The cost: no single file shows the full assembled prompt; the assembly function and its tests carry that view.

**Generated `Deserialize` params structs over schemars doc-comments.** Today parameter schemas derive from `#[derive(JsonSchema)]` structs whose doc comments are the model-visible descriptions — prompt text living in Rust doc comments, invisible to the prompt tree. Moving descriptions to `tool_schema` frontmatter and generating both the JSON schema *and* the deserialization struct from it makes the file the single source of truth with zero drift surface between schema and struct; the schemars dependency and its post-hoc `$schema`/`title` stripping go away. The alternative — keeping hand-written structs beside frontmatter schemas — leaves two artifacts to drift apart with only convention holding them together, which is exactly the failure mode this crate exists to close.

**Shared params files over per-tool duplication.** A dozen lifecycle tools deliberately share one `{name}` parameter shape today, via one struct whose stated purpose is preventing the surfaces from drifting into subtly different contracts. One-file-per-tool would copy that parameter definition a dozen times — reintroducing at the file layer the drift the shared struct exists to prevent. `type: params` files preserve the sharing: one definition, one generated struct, dispatch code unchanged in shape. The rejected alternative (accepting duplication as "files are cheap") fails the design's own reasoning; the more elaborate alternative (schema inheritance/merging) buys generality nothing needs yet.

**Wire-name override over forced 1:1 id/name.** Two surfaces deliberately expose the same tool name with different description text (a terse in-game variant). Since ids must be unique, the wire name must be decoupled; defaulting it to snake_case(id) keeps the common case zero-config while the explicit override marks variants as intentional.

**Per-crate prompt tree over per-workspace.** The consuming crate's build script, source tree (for `verify_annotations`), and generated module are all crate-scoped; anchoring `prompts/` at the crate root keeps every path relationship local and lets future crates adopt the pattern independently.

## External touchpoints

- **Consumer build scripts.** The crate's codegen face is invoked from a consumer's `build.rs`. Contract: one call taking the prompt-directory path; on failure it returns an error (build scripts report via `Result` from main, not panics — the workspace lint regime denies `panic`/`exit` and lints build scripts too); on success `prompts.rs` is in the build output directory and the prompt tree is registered for rerun-on-change.
- **The OpenAI-compatible chat API (via the bot's existing client).** Generated tool specs must convert losslessly into the bot's existing private wire types (function name, description string, `serde_json::Value` parameters schema). The bot owns this glue: one conversion — a `From` impl or equivalent helper in one place — from the prompt-lib `ToolSpec` to its wire type; the LLM client's own types stay untouched. Schemas must omit `$schema` and `title` (some providers reject them). The generated params structs must deserialize the `arguments` JSON string the API returns for tool calls.
- **Workspace lint regime.** Generated code and the build-script path must be clean under the workspace's deny-level clippy set or carry scoped `#[expect(..., reason)]` emitted by codegen.
- **Existing YAML tooling.** Frontmatter parsing uses the workspace's established YAML dependency (the maintained serde_yaml fork already used for the game catalog); no new YAML parser enters the tree, and the codegen feature gate keeps it out of the shipped binary's dependency graph.
- **CI / gate / deploy: no changes.** Prompts compile into the binary during the normal gated image build; no Dockerfile copies, no chart values, no new env vars, no volume mounts.
- **The agent maintenance contract** touches the repo's CLAUDE.md, which gains the convention section when the first prompts land.

## Integration with existing system

- **Delivery model:** the entire effort — crate, codegen, and migration — executes on one dedicated feature branch, with per-phase commits landing on that branch. It merges to `main` once, at completion, producing a single CI build/gate-sign/deploy cycle instead of one per phase. Per-phase verification is therefore local (build, lints, tests), never a deploy.
- **The bot crate gains** a `prompts/` tree, a build script (the workspace's first — a one-line call into the codegen face), the include of the generated module, the `ToolSpec` conversion glue, and the conventional annotations test. Its manifest lists `prompt-lib` three times — under `[build-dependencies]` with the codegen feature, `[dependencies]` with defaults, and `[dev-dependencies]` with the verify feature — the standard Cargo pattern for a crate whose generated output references its own runtime types.
- **System prompts:** the Discord prompt-assembly function keeps its shape — it selects and concatenates tier-gated blocks — but each block's text becomes a prompt file rendered through its accessor instead of a pushed string literal, with the assembly code owning the inter-block separators. Behavior-level tests that assert on substrings of the assembled prompt stay green, since the assembled output is text-identical. The in-game system prompt and the deferred-task batch prompt migrate the same way (framing prose in files; the numbered task list joined in code enters as a variable).
- **Tool definitions:** the tool-definition modules on both surfaces swap hand-written name/description literals and schemars-derived schemas for generated `spec()` calls; dispatch `match` arms swap string constants for generated `NAME` consts; dispatch deserialization swaps hand-written params structs for the generated ones (shared params files preserving today's deliberate struct sharing). Domain narrowing (e.g. the deferred-task condition enum, unsigned line counts) becomes explicit conversions at the dispatch boundary, failing back to the model as tool-result text. Once migration completes, the schemars dependency and the schema-stripping helper are removed.
- **Tool-result prose:** per the tool-result formatting decision (ADR-008), every tool formats its own result privately and there is one format everywhere — only Gary's final reply adapts per surface. The fixed model-facing phrases in those formatters and in dispatch fallbacks (empty-list copy, refusal lines, error copy) migrate to prompt files under the scope rule; joining and per-item data formatting stay in code and flow in through variables, with value-level fallbacks documented in the variable annotations. Identical model-facing text used on both surfaces consolidates into *one* prompt file with multiple `used_by` entries — ADR-008's duplication choice governs the Rust formatters, not the prompt tree; the tree exists precisely to give repeated text one home.
- **Coexistence during migration:** generated accessors are purely additive, so prompts migrate incrementally — each migrated prompt deletes its literal at the same call site in the same change, and the system runs mixed (some literals, some accessors) at every intermediate point on the feature branch. Migration is *text-preserving*: the bytes sent to the model before and after are identical modulo variable extraction (and the schema tightening noted above). Done means no model-visible prose remains as a string literal in bot source.
- **Explicitly untouched:** the LLM client and session loop (message types, tool-call plumbing), the supervisor and control-api crates, model/sampling configuration, and all handling of model output. Tool *redesigns* surfaced during this work (the `server_status`/`list_servers` overlap, tracked as issue #44) happen outside the migration.

## Non-goals

Model and sampling configuration; API clients or transport; provider abstractions; a standalone lint binary (subsumed by build-time validation and the conventional test); prompt versioning, A/B testing, or analytics; conditional or loop syntax in the file format; schema inheritance or merging between params files and inline schemas.

## Deferred (explicitly)

- **Runtime file loading** (the draft spec's default, now inverted): an opt-in mode reading bodies from disk with a startup drift guard, if live prompt tuning without rebuild ever becomes worth its wiring.
- **Arrays and nested objects in `tool_schema`**, when a tool actually needs them.
- **A dynamic string-keyed render path** (`render(id, vars)`), if a use case ever can't go through typed accessors.
- **Extraction and publication** out of the workspace once the format survives real use here.

## Open questions

None.
