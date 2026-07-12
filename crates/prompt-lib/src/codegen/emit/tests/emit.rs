use std::path::PathBuf;

use serde_json::json;

/// The generated module, compiled from the checked-in golden. Including it here
/// compiles the emitted code under the crate's real lint regime, and lets the
/// behavior tests below call `render`/`spec`/`NAME` and deserialize the params
/// structs.
mod generated {
    include!("../../tests/fixtures/expected_prompts.rs");
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/codegen/tests/fixtures/valid")
}

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/codegen/tests/fixtures/expected_prompts.rs")
}

#[test]
#[ignore = "run to regenerate the golden: cargo test -p grizzly-prompt-lib --features codegen regenerate_golden -- --ignored"]
fn regenerate_golden() {
    let source = super::generate(&fixtures_dir()).unwrap();
    std::fs::write(golden_path(), source).unwrap();
}

/// The checked-in golden must match what the emitter produces today. If this
/// fails, the emitter changed — rerun `regenerate_golden` and review the diff.
#[test]
fn golden_is_current() {
    let generated = super::generate(&fixtures_dir()).unwrap();
    let golden = std::fs::read_to_string(golden_path()).unwrap();
    assert_eq!(generated, golden, "golden is stale; regenerate it");
}

#[test]
fn render_inserts_variable_verbatim() {
    let out = generated::Greeting { name: "Ada" }.render();
    assert_eq!(
        out,
        "Hello Ada, I'm Gary. Tell me what you'd like to do with your servers."
    );
}

#[test]
fn render_of_zero_variable_prompt_is_the_body() {
    let out = generated::Disclaimer::render();
    assert_eq!(
        out,
        "Gary can make mistakes. Double-check anything important before relying on it."
    );
}

#[test]
fn name_derives_snake_case_from_id() {
    assert_eq!(generated::EditFile::NAME, "edit_file");
    assert_eq!(generated::Ping::NAME, "ping");
    assert_eq!(generated::StartServer::NAME, "start_server");
}

#[test]
fn inline_schema_carries_types_required_enum_and_optional() {
    let spec = generated::EditFile::spec();
    assert_eq!(spec.name, "edit_file");
    assert_eq!(
        spec.description,
        "Edit a configuration file on the server, then verify and roll back on failure."
    );
    assert_eq!(
        spec.parameters,
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "path to the file on the server's data volume" },
                "mode": {
                    "type": "string",
                    "description": "whether to replace the whole file or append to it",
                    "enum": ["replace", "append"]
                },
                "count": { "type": "integer", "description": "number of lines to write; omit to write the whole payload" }
            },
            "required": ["path", "mode"],
            "additionalProperties": false
        })
    );
}

#[test]
fn zero_parameter_tool_has_empty_object_schema() {
    let spec = generated::Ping::spec();
    assert_eq!(
        spec.parameters,
        json!({ "type": "object", "properties": {}, "additionalProperties": false })
    );
}

#[test]
fn params_from_tool_embeds_the_shared_schema() {
    // StartServer shares NameParams; its spec carries that schema and it emits no
    // params struct of its own (only NameParams exists in the golden).
    let spec = generated::StartServer::spec();
    assert_eq!(
        spec.parameters,
        json!({
            "type": "object",
            "properties": { "name": { "type": "string", "description": "the server name to act on" } },
            "required": ["name"],
            "additionalProperties": false
        })
    );
}

#[test]
fn generated_params_struct_deserializes_all_fields() {
    let params: generated::EditFileParams =
        serde_json::from_str(r#"{"path":"world/server.properties","mode":"replace","count":3}"#)
            .unwrap();
    assert_eq!(params.path, "world/server.properties");
    assert_eq!(params.count, Some(3));
    match params.mode {
        generated::EditFileMode::Replace => {}
        generated::EditFileMode::Append => panic!("expected replace"),
    }
}

#[test]
fn optional_param_defaults_to_none_when_omitted() {
    let params: generated::EditFileParams =
        serde_json::from_str(r#"{"path":"a","mode":"append"}"#).unwrap();
    assert_eq!(params.count, None);
    match params.mode {
        generated::EditFileMode::Append => {}
        generated::EditFileMode::Replace => panic!("expected append"),
    }
}

#[test]
fn shared_params_struct_deserializes() {
    let params: generated::NameParams = serde_json::from_str(r#"{"name":"grizzly-mc"}"#).unwrap();
    assert_eq!(params.name, "grizzly-mc");
}

#[test]
fn emit_to_writes_prompts_rs() {
    let out = tempfile::tempdir().unwrap();
    super::emit_to(&fixtures_dir(), out.path()).unwrap();
    let written = std::fs::read_to_string(out.path().join("prompts.rs")).unwrap();
    assert_eq!(written, super::generate(&fixtures_dir()).unwrap());
}
