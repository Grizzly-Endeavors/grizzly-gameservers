use super::*;

#[test]
fn no_args_schema_is_an_empty_object() {
    assert_eq!(
        no_args_schema(),
        serde_json::json!({ "type": "object", "properties": {} })
    );
}

#[test]
fn params_schema_strips_provider_rejected_keys() {
    let schema = params_schema::<NameParams>();
    let object = schema.as_object().expect("schema is a JSON object");
    assert!(
        !object.contains_key("$schema"),
        "$schema must be stripped — some providers reject it"
    );
    assert!(
        !object.contains_key("title"),
        "title must be stripped — some providers reject it"
    );
}

#[test]
fn name_params_schema_describes_the_name_field() {
    let schema = params_schema::<NameParams>();
    assert_eq!(
        schema.pointer("/type"),
        Some(&serde_json::Value::from("object"))
    );
    assert_eq!(
        schema.pointer("/properties/name/type"),
        Some(&serde_json::Value::from("string"))
    );
    assert_eq!(
        schema.pointer("/required"),
        Some(&serde_json::json!(["name"])),
        "name is required"
    );
}

#[test]
fn name_params_round_trips_from_tool_arguments() {
    let parsed: NameParams = serde_json::from_str(r#"{"name":"mc-abc123"}"#).unwrap();
    assert_eq!(parsed.name, "mc-abc123");
}
