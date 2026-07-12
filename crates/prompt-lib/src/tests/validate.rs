use std::path::PathBuf;

use super::*;

/// Write each `(relative_path, content)` into a fresh tempdir and validate it.
fn load_tree(files: &[(&str, &str)]) -> Result<PromptTree, PromptError> {
    let dir = tempfile::tempdir().unwrap();
    for (rel, content) in files {
        let path = dir.path().join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, content).unwrap();
    }
    load(dir.path())
}

fn err_of(files: &[(&str, &str)]) -> PromptError {
    load_tree(files).unwrap_err()
}

const GREETING: &str = "---
id: Greeting
type: prompt
annotations:
  sent_when: at conversation start
  used_by:
    - file: src/discord.rs
      function: greet
  variables:
    name:
      source: the user record
      contents: the display name
  reasoning:
    - opens every conversation
---
Hello {{name}}.";

const EDIT_FILE: &str = "---
id: EditFile
type: tool
tool_schema:
  path:
    type: string
    description: the file to edit
  mode:
    type: enum
    description: how to edit
    values:
      - replace
      - append
  count:
    type: integer
    description: number of lines
    optional: true
annotations:
  sent_when: offered to managers and admins
  used_by:
    - file: src/tools.rs
      function: dispatch
  reasoning:
    - lets Gary edit server config
---
Edit a file on the server.";

const NAME_PARAMS: &str = "---
id: NameParams
type: params
tool_schema:
  name:
    type: string
    description: the server name
annotations:
  used_by:
    - file: src/tools.rs
      function: dispatch
  reasoning:
    - shared by lifecycle tools
---
";

const START_SERVER: &str = "---
id: StartServer
type: tool
params_from: NameParams
annotations:
  sent_when: offered to managers
  used_by:
    - file: src/tools.rs
      function: dispatch
  reasoning:
    - starts a stopped server
---
Start a server.";

// --- Accepting cases ---

#[test]
fn accepts_prompt_with_variable() {
    let tree = load_tree(&[("Greeting.md", GREETING)]).unwrap();
    let PromptKind::Prompt { body, variables } =
        &tree.files.get("Greeting").expect("file present").kind
    else {
        panic!("expected a prompt");
    };
    assert_eq!(variables.len(), 1, "one variable");
    assert_eq!(variables.first().expect("one variable").name, "name");
    assert!(
        body.iter()
            .any(|s| matches!(s, BodySegment::Placeholder(n) if n == "name")),
        "body carries the placeholder"
    );
}

#[test]
fn accepts_tool_with_inline_enum_and_optional() {
    let tree = load_tree(&[("EditFile.md", EDIT_FILE)]).unwrap();
    let PromptKind::Tool {
        wire_name,
        name_explicit,
        schema: ToolSchemaRef::Inline(schema),
        ..
    } = &tree.files.get("EditFile").expect("file present").kind
    else {
        panic!("expected an inline tool");
    };
    assert_eq!(wire_name, "edit_file", "wire name defaults to snake_case");
    assert!(!name_explicit);
    assert_eq!(schema.params.len(), 3);
    let mode = &schema
        .params
        .iter()
        .find(|(n, _)| n == "mode")
        .expect("mode param")
        .1;
    assert!(matches!(&mode.ty, ParamType::Enum { values } if values == &["replace", "append"]));
    let count = &schema
        .params
        .iter()
        .find(|(n, _)| n == "count")
        .expect("count param")
        .1;
    assert!(count.optional, "count is optional");
}

#[test]
fn accepts_params_and_referencing_tool() {
    let tree = load_tree(&[
        ("NameParams.md", NAME_PARAMS),
        ("StartServer.md", START_SERVER),
    ])
    .unwrap();
    assert!(matches!(
        &tree.files.get("StartServer").expect("file present").kind,
        PromptKind::Tool { schema: ToolSchemaRef::Shared(id), .. } if id == "NameParams"
    ));
    assert!(matches!(
        &tree.files.get("NameParams").expect("file present").kind,
        PromptKind::Params { .. }
    ));
}

#[test]
fn accepts_zero_parameter_tool() {
    let ping = "---
id: Ping
type: tool
tool_schema: {}
annotations:
  sent_when: always
  used_by:
    - file: src/x.rs
      function: f
  reasoning:
    - health check
---
Ping the server.";
    let tree = load_tree(&[("Ping.md", ping)]).unwrap();
    assert!(matches!(
        &tree.files.get("Ping").expect("file present").kind,
        PromptKind::Tool { schema: ToolSchemaRef::Inline(s), .. } if s.params.is_empty()
    ));
}

#[test]
fn accepts_explicit_wire_name_override_on_both_sides() {
    // Two tools sharing a wire name is allowed when both declare it explicitly.
    let a = "---
id: EditFileDiscord
type: tool
name: edit_file
tool_schema: {}
annotations:
  sent_when: discord surface
  used_by:
    - file: src/discord.rs
      function: f
  reasoning:
    - discord variant
---
Edit a file.";
    let b = "---
id: EditFileInGame
type: tool
name: edit_file
tool_schema: {}
annotations:
  sent_when: in-game surface
  used_by:
    - file: src/ingame.rs
      function: f
  reasoning:
    - terser in-game variant
---
Edit a file.";
    let tree = load_tree(&[("EditFileDiscord.md", a), ("EditFileInGame.md", b)]).unwrap();
    assert_eq!(tree.files.len(), 2);
}

#[test]
fn accepts_empty_tree() {
    let tree = load_tree(&[]).unwrap();
    assert!(tree.files.is_empty());
}

// --- Rejecting cases: identity and type ---

#[test]
fn rejects_missing_id() {
    let content = "---\ntype: prompt\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\nhi";
    assert!(matches!(
        err_of(&[("Greeting.md", content)]),
        PromptError::MissingId { .. }
    ));
}

#[test]
fn rejects_bad_id_grammar() {
    let content = "---\nid: editFile\ntype: prompt\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\nhi";
    assert!(matches!(
        err_of(&[("editFile.md", content)]),
        PromptError::IdGrammar { .. }
    ));
}

#[test]
fn rejects_id_filename_mismatch() {
    assert!(matches!(
        err_of(&[("Other.md", GREETING)]),
        PromptError::IdFilenameMismatch { .. }
    ));
}

#[test]
fn rejects_missing_type() {
    let content = "---\nid: Greeting\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\nhi";
    assert!(matches!(
        err_of(&[("Greeting.md", content)]),
        PromptError::MissingType { .. }
    ));
}

#[test]
fn rejects_unknown_type() {
    let content = "---\nid: Greeting\ntype: widget\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\nhi";
    assert!(matches!(
        err_of(&[("Greeting.md", content)]),
        PromptError::UnknownType { .. }
    ));
}

// --- Rejecting cases: per-type key legality ---

#[test]
fn rejects_disallowed_key_on_prompt() {
    let content = "---\nid: Greeting\ntype: prompt\nname: nope\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\nhi";
    assert!(matches!(
        err_of(&[("Greeting.md", content)]),
        PromptError::KeyNotAllowed { .. }
    ));
}

#[test]
fn rejects_tool_with_both_schema_sources() {
    let content = "---\nid: EditFile\ntype: tool\nparams_from: NameParams\ntool_schema: {}\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\nEdit.";
    assert!(matches!(
        err_of(&[("EditFile.md", content), ("NameParams.md", NAME_PARAMS)]),
        PromptError::SchemaSourceAmbiguous { .. }
    ));
}

#[test]
fn rejects_tool_with_no_schema_source() {
    let content = "---\nid: EditFile\ntype: tool\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\nEdit.";
    assert!(matches!(
        err_of(&[("EditFile.md", content)]),
        PromptError::SchemaSourceMissing { .. }
    ));
}

// --- Rejecting cases: body and placeholders ---

#[test]
fn rejects_body_whitespace() {
    let content = "---\nid: Greeting\ntype: prompt\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\n  indented body";
    assert!(matches!(
        err_of(&[("Greeting.md", content)]),
        PromptError::BodyWhitespace { .. }
    ));
}

#[test]
fn rejects_placeholder_in_tool_body() {
    let content = "---\nid: EditFile\ntype: tool\ntool_schema: {}\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\nEdit {{path}} now.";
    assert!(matches!(
        err_of(&[("EditFile.md", content)]),
        PromptError::PlaceholderInStaticText { .. }
    ));
}

#[test]
fn rejects_placeholder_in_param_description() {
    let content = "---\nid: EditFile\ntype: tool\ntool_schema:\n  path:\n    type: string\n    description: the {{thing}} to edit\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\nEdit.";
    assert!(matches!(
        err_of(&[("EditFile.md", content)]),
        PromptError::PlaceholderInStaticText { .. }
    ));
}

#[test]
fn rejects_unterminated_placeholder() {
    let content = "---\nid: Greeting\ntype: prompt\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\nHello {{name";
    assert!(matches!(
        err_of(&[("Greeting.md", content)]),
        PromptError::UnterminatedPlaceholder { .. }
    ));
}

#[test]
fn rejects_illegal_placeholder_name() {
    let content = "---\nid: Greeting\ntype: prompt\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  variables:\n    Name:\n      source: s\n      contents: c\n  reasoning:\n    - c\n---\nHello {{Name}}.";
    assert!(matches!(
        err_of(&[("Greeting.md", content)]),
        PromptError::PlaceholderName { .. }
    ));
}

#[test]
fn rejects_undeclared_placeholder() {
    let content = "---\nid: Greeting\ntype: prompt\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  variables:\n    other:\n      source: s\n      contents: c\n  reasoning:\n    - c\n---\nHello {{game}}.";
    assert!(matches!(
        err_of(&[("Greeting.md", content)]),
        PromptError::UndeclaredPlaceholder { .. }
    ));
}

#[test]
fn rejects_unused_variable() {
    let content = "---\nid: Greeting\ntype: prompt\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  variables:\n    name:\n      source: s\n      contents: c\n    extra:\n      source: s\n      contents: c\n  reasoning:\n    - c\n---\nHello {{name}}.";
    assert!(matches!(
        err_of(&[("Greeting.md", content)]),
        PromptError::UnusedVariable { .. }
    ));
}

#[test]
fn rejects_variables_without_placeholders() {
    let content = "---\nid: Greeting\ntype: prompt\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  variables:\n    name:\n      source: s\n      contents: c\n  reasoning:\n    - c\n---\nno placeholders here";
    assert!(matches!(
        err_of(&[("Greeting.md", content)]),
        PromptError::VariablesKeyMismatch { .. }
    ));
}

#[test]
fn rejects_placeholders_without_variables() {
    let content = "---\nid: Greeting\ntype: prompt\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\nHello {{name}}.";
    assert!(matches!(
        err_of(&[("Greeting.md", content)]),
        PromptError::VariablesKeyMismatch { .. }
    ));
}

// --- Rejecting cases: tool_schema parameters ---

#[test]
fn rejects_unsupported_param_type() {
    let content = "---\nid: EditFile\ntype: tool\ntool_schema:\n  tags:\n    type: array\n    description: a list\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\nEdit.";
    assert!(matches!(
        err_of(&[("EditFile.md", content)]),
        PromptError::ParamType { .. }
    ));
}

#[test]
fn rejects_param_without_description() {
    let content = "---\nid: EditFile\ntype: tool\ntool_schema:\n  path:\n    type: string\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\nEdit.";
    assert!(matches!(
        err_of(&[("EditFile.md", content)]),
        PromptError::ParamDescription { .. }
    ));
}

#[test]
fn rejects_params_without_schema() {
    let content = "---\nid: NameParams\ntype: params\nannotations:\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\n";
    assert!(matches!(
        err_of(&[("NameParams.md", content)]),
        PromptError::ParamsSchemaMissing { .. }
    ));
}

#[test]
fn rejects_enum_without_values() {
    let content = "---\nid: EditFile\ntype: tool\ntool_schema:\n  mode:\n    type: enum\n    description: how\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\nEdit.";
    assert!(matches!(
        err_of(&[("EditFile.md", content)]),
        PromptError::EnumValuesMissing { .. }
    ));
}

#[test]
fn rejects_bad_enum_value_grammar() {
    let content = "---\nid: EditFile\ntype: tool\ntool_schema:\n  mode:\n    type: enum\n    description: how\n    values:\n      - Bad-Value\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\nEdit.";
    assert!(matches!(
        err_of(&[("EditFile.md", content)]),
        PromptError::EnumValueGrammar { .. }
    ));
}

#[test]
fn rejects_values_on_non_enum() {
    let content = "---\nid: EditFile\ntype: tool\ntool_schema:\n  path:\n    type: string\n    description: the path\n    values:\n      - a\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\nEdit.";
    assert!(matches!(
        err_of(&[("EditFile.md", content)]),
        PromptError::ValuesOnNonEnum { .. }
    ));
}

// --- Rejecting cases: annotations ---

#[test]
fn rejects_missing_annotations() {
    let content = "---\nid: Greeting\ntype: prompt\n---\nhi";
    assert!(matches!(
        err_of(&[("Greeting.md", content)]),
        PromptError::MissingAnnotation { .. }
    ));
}

#[test]
fn rejects_empty_used_by_list() {
    let content = "---\nid: Greeting\ntype: prompt\nannotations:\n  sent_when: x\n  used_by: []\n  reasoning:\n    - c\n---\nhi";
    assert!(matches!(
        err_of(&[("Greeting.md", content)]),
        PromptError::EmptyAnnotation { .. }
    ));
}

#[test]
fn rejects_blank_reasoning_note() {
    let content = "---\nid: Greeting\ntype: prompt\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - \"   \"\n---\nhi";
    assert!(matches!(
        err_of(&[("Greeting.md", content)]),
        PromptError::EmptyAnnotation { .. }
    ));
}

#[test]
fn rejects_sent_when_on_params() {
    let content = "---\nid: NameParams\ntype: params\ntool_schema:\n  name:\n    type: string\n    description: the name\nannotations:\n  sent_when: nope\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\n";
    assert!(matches!(
        err_of(&[("NameParams.md", content), ("StartServer.md", START_SERVER)]),
        PromptError::ForbiddenAnnotation { .. }
    ));
}

#[test]
fn rejects_variables_on_tool() {
    let content = "---\nid: EditFile\ntype: tool\ntool_schema: {}\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  variables:\n    name:\n      source: s\n      contents: c\n  reasoning:\n    - c\n---\nEdit.";
    assert!(matches!(
        err_of(&[("EditFile.md", content)]),
        PromptError::ForbiddenAnnotation { .. }
    ));
}

// --- Rejecting cases: cross-file ---

#[test]
fn rejects_duplicate_id() {
    assert!(matches!(
        err_of(&[("a/Greeting.md", GREETING), ("b/Greeting.md", GREETING)]),
        PromptError::DuplicateId { .. }
    ));
}

#[test]
fn rejects_unknown_params_ref() {
    let content = "---\nid: StartServer\ntype: tool\nparams_from: Missing\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\nStart.";
    assert!(matches!(
        err_of(&[("StartServer.md", content)]),
        PromptError::UnknownParamsRef { .. }
    ));
}

#[test]
fn rejects_unreferenced_params() {
    assert!(matches!(
        err_of(&[("NameParams.md", NAME_PARAMS)]),
        PromptError::UnreferencedParams { .. }
    ));
}

#[test]
fn rejects_wire_name_collision() {
    // A derived wire name (foo) collides with another tool's explicit override.
    let foo = "---\nid: Foo\ntype: tool\ntool_schema: {}\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\nFoo.";
    let bar = "---\nid: Bar\ntype: tool\nname: foo\ntool_schema: {}\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\nBar.";
    assert!(matches!(
        err_of(&[("Foo.md", foo), ("Bar.md", bar)]),
        PromptError::WireNameCollision { .. }
    ));
}

#[test]
fn rejects_derived_name_collision() {
    // Tool `Edit` with an inline schema synthesizes `EditParams`, colliding with
    // a params file of that id (which is kept referenced so the unreferenced
    // rule does not fire first).
    let edit = "---\nid: Edit\ntype: tool\ntool_schema:\n  target:\n    type: string\n    description: the target\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\nEdit.";
    let edit_params = "---\nid: EditParams\ntype: params\ntool_schema:\n  name:\n    type: string\n    description: the name\nannotations:\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\n";
    let use_it = "---\nid: UseIt\ntype: tool\nparams_from: EditParams\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\nUse it.";
    assert!(matches!(
        err_of(&[
            ("Edit.md", edit),
            ("EditParams.md", edit_params),
            ("UseIt.md", use_it)
        ]),
        PromptError::DerivedNameCollision { .. }
    ));
}

// --- Integration: the committed all-three-types fixture tree ---

#[test]
fn loads_committed_valid_tree() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/tests/fixtures/valid");
    let tree = load(&dir).unwrap();
    assert_eq!(tree.files.len(), 4, "prompt + two tools + params");

    assert!(matches!(
        &tree.files.get("Greeting").expect("file present").kind,
        PromptKind::Prompt { variables, .. } if variables.len() == 1
    ));
    assert!(matches!(
        &tree.files.get("EditFile").expect("file present").kind,
        PromptKind::Tool {
            schema: ToolSchemaRef::Inline(_),
            ..
        }
    ));
    assert!(matches!(
        &tree.files.get("StartServer").expect("file present").kind,
        PromptKind::Tool { schema: ToolSchemaRef::Shared(id), .. } if id == "NameParams"
    ));
    assert!(matches!(
        &tree.files.get("NameParams").expect("file present").kind,
        PromptKind::Params { .. }
    ));
}

#[test]
fn error_display_names_the_relative_file_and_rule() {
    let content = "---\nid: Greeting\ntype: prompt\nannotations:\n  sent_when: x\n  used_by:\n    - file: a\n      function: b\n  reasoning:\n    - c\n---\nHello {{name}}.";
    let message = err_of(&[("gary/Greeting.md", content)]).to_string();
    assert!(
        message.contains("gary/Greeting.md"),
        "names the file: {message}"
    );
    assert!(message.contains("variables"), "names the rule: {message}");
}
