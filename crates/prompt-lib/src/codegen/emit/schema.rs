//! Assemble a tool's JSON parameter schema as a `serde_json::json!` expression.
//!
//! The schema is built at the consumer's compile time by the generated
//! `spec()`; here we only emit the macro invocation. Parameter order follows the
//! source `tool_schema` (design §"Assembled tool schema"): `object` with typed,
//! described `properties`, a `required` array (omitted when empty), and
//! `additionalProperties: false`. No `$schema`, no `title`.

use super::super::model::{Param, ParamType, ToolSchema};
use super::Code;
use super::rust::{json_type, str_lit};

/// Emit `parameters: …json!({ … }),` — a struct-field line whose value is the
/// assembled schema for `schema`.
pub(super) fn emit_schema(code: &mut Code, schema: &ToolSchema) {
    code.open("parameters: grizzly_prompt_lib::__private::serde_json::json!({");
    code.line("\"type\": \"object\",");
    emit_properties(code, schema);
    emit_required(code, schema);
    code.line("\"additionalProperties\": false");
    code.close("}),");
}

/// The `properties` object, one entry per parameter in source order.
fn emit_properties(code: &mut Code, schema: &ToolSchema) {
    if schema.params.is_empty() {
        code.line("\"properties\": {},");
        return;
    }
    code.open("\"properties\": {");
    let last = schema.params.len() - 1;
    for (idx, (name, param)) in schema.params.iter().enumerate() {
        emit_property(code, name, param, idx == last);
    }
    code.close("},");
}

/// One `"name": { "type", "description"[, "enum"] }` entry, with a trailing
/// comma unless it is the last property.
fn emit_property(code: &mut Code, name: &str, param: &Param, is_last: bool) {
    code.open(&format!("{}: {{", str_lit(name)));
    code.line(&format!("\"type\": {},", str_lit(json_type(&param.ty))));
    match &param.ty {
        ParamType::Enum { values } => {
            code.line(&format!(
                "\"description\": {},",
                str_lit(&param.description)
            ));
            let list = values
                .iter()
                .map(|v| str_lit(v))
                .collect::<Vec<_>>()
                .join(", ");
            code.line(&format!("\"enum\": [{list}]"));
        }
        ParamType::String | ParamType::Integer | ParamType::Number | ParamType::Boolean => {
            code.line(&format!("\"description\": {}", str_lit(&param.description)));
        }
    }
    code.close(if is_last { "}" } else { "}," });
}

/// The `required` array of every non-optional parameter. Omitted entirely when
/// empty (a schema with only optional or zero parameters).
fn emit_required(code: &mut Code, schema: &ToolSchema) {
    let required = schema
        .params
        .iter()
        .filter(|(_, param)| !param.optional)
        .map(|(name, _)| str_lit(name))
        .collect::<Vec<_>>();
    if !required.is_empty() {
        code.line(&format!("\"required\": [{}],", required.join(", ")));
    }
}
