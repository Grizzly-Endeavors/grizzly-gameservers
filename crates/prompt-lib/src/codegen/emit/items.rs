//! Emit the Rust items for each prompt file: prompt render structs, tool unit
//! structs, and `Deserialize` params/enum structs (design §"Generated code
//! contract").

use super::super::model::{
    BodySegment, ParamType, PromptKind, PromptTree, ToolSchema, ToolSchemaRef, Variable,
};
use super::Code;
use super::rust::{enum_type_name, field_type, str_lit};
use super::schema::emit_schema;

/// A prompt: a struct with one borrowed field per variable and an infallible
/// `render` that interleaves body literals with those fields.
pub(super) fn emit_prompt(code: &mut Code, id: &str, body: &[BodySegment], variables: &[Variable]) {
    if variables.is_empty() {
        code.line(&format!("pub struct {id};"));
    } else {
        code.open(&format!("pub struct {id}<'a> {{"));
        for var in variables {
            code.line(&format!("pub {}: &'a str,", var.name));
        }
        code.close("}");
    }
    code.blank();

    let head = if variables.is_empty() {
        format!("impl {id} {{")
    } else {
        format!("impl {id}<'_> {{")
    };
    code.open(&head);
    code.line("#[must_use]");
    // A no-variable prompt has nothing to consume, so its `render` is an
    // associated function; a prompt with variables consumes `self`.
    let signature = if variables.is_empty() {
        "pub fn render() -> String {"
    } else {
        "pub fn render(self) -> String {"
    };
    code.open(signature);
    emit_render_body(code, body);
    code.close("}");
    code.close("}");
}

/// The body of `render`. A body with no placeholders is a single owned literal;
/// otherwise a `String` is built by pushing each literal and field in turn.
fn emit_render_body(code: &mut Code, body: &[BodySegment]) {
    let has_placeholder = body
        .iter()
        .any(|seg| matches!(seg, BodySegment::Placeholder(_)));
    if !has_placeholder {
        let literal = match body.first() {
            Some(BodySegment::Literal(text)) => format!("String::from({})", str_lit(text)),
            Some(BodySegment::Placeholder(_)) | None => "String::new()".to_owned(),
        };
        code.line(&literal);
        return;
    }
    code.line("let mut out = String::new();");
    for seg in body {
        match seg {
            BodySegment::Literal(text) => code.line(&format!("out.push_str({});", str_lit(text))),
            BodySegment::Placeholder(name) => code.line(&format!("out.push_str(self.{name});")),
        }
    }
    code.line("out");
}

/// A tool: a unit struct with `NAME` and `spec()`, plus its own params struct
/// when it defines an inline schema with at least one parameter.
pub(super) fn emit_tool(
    code: &mut Code,
    id: &str,
    wire_name: &str,
    description: &str,
    schema: &ToolSchemaRef,
    tree: &PromptTree,
) {
    let resolved = resolve_schema(schema, tree);
    code.line(&format!("pub struct {id};"));
    code.blank();
    code.open(&format!("impl {id} {{"));
    code.line(&format!(
        "pub const NAME: &'static str = {};",
        str_lit(wire_name)
    ));
    code.blank();
    code.line("#[must_use]");
    code.open("pub fn spec() -> grizzly_prompt_lib::ToolSpec {");
    code.open("grizzly_prompt_lib::ToolSpec {");
    code.line("name: Self::NAME,");
    code.line(&format!("description: {},", str_lit(description)));
    emit_schema(code, resolved);
    code.close("}");
    code.close("}");
    code.close("}");

    if let ToolSchemaRef::Inline(inline) = schema
        && !inline.params.is_empty()
    {
        code.blank();
        // Inline tools synthesize `<Id>Params`, but their enums still key off the
        // file id (`<Id><Param>`) — matching `register_derived_names`.
        emit_params_struct(code, &format!("{id}Params"), id, inline);
    }
}

/// A shared params file: only the `Deserialize` struct (and any enums); tools
/// reference it, so it has no `spec()` of its own. A params file names its struct
/// and its enums both from its own id.
pub(super) fn emit_params(code: &mut Code, id: &str, schema: &ToolSchema) {
    emit_params_struct(code, id, id, schema);
}

/// Resolve a tool's schema reference to the concrete schema whose parameters go
/// into the JSON spec. A `params_from` reference is looked up in the tree; the
/// referenced file is guaranteed to exist and be a params file by validation.
fn resolve_schema<'a>(schema: &'a ToolSchemaRef, tree: &'a PromptTree) -> &'a ToolSchema {
    match schema {
        ToolSchemaRef::Inline(inline) => inline,
        ToolSchemaRef::Shared(target) => match tree.files.get(target).map(|file| &file.kind) {
            Some(PromptKind::Params { schema: shared }) => shared,
            Some(PromptKind::Prompt { .. } | PromptKind::Tool { .. }) | None => &EMPTY_SCHEMA,
        },
    }
}

/// Fallback for the unreachable case where a `params_from` target is not a
/// validated params file; validation rejects that before codegen runs.
static EMPTY_SCHEMA: ToolSchema = ToolSchema { params: Vec::new() };

/// The shared body of a params-bearing struct: fields with serde defaults on
/// optionals, followed by one generated enum per `enum` parameter. `struct_name`
/// is what the struct is called; `enum_owner` is the id enum names key off (the
/// two differ for inline tools: `EditFileParams` struct, `EditFileMode` enum).
fn emit_params_struct(code: &mut Code, struct_name: &str, enum_owner: &str, schema: &ToolSchema) {
    code.line("#[derive(serde::Deserialize)]");
    code.open(&format!("pub struct {struct_name} {{"));
    for (name, param) in &schema.params {
        if param.optional {
            code.line("#[serde(default)]");
        }
        code.line(&format!(
            "pub {name}: {},",
            field_type(param, enum_owner, name)
        ));
    }
    code.close("}");

    for (name, param) in &schema.params {
        if let ParamType::Enum { values } = &param.ty {
            code.blank();
            emit_enum(code, enum_owner, name, values);
        }
    }
}

/// One generated enum: a variant per value, `PascalCase`d and serde-renamed back
/// to the exact wire string.
fn emit_enum(code: &mut Code, owner_id: &str, param_name: &str, values: &[String]) {
    code.line("#[derive(serde::Deserialize)]");
    code.open(&format!(
        "pub enum {} {{",
        enum_type_name(owner_id, param_name)
    ));
    for value in values {
        code.line(&format!("#[serde(rename = {})]", str_lit(value)));
        code.line(&format!("{},", super::super::ident::snake_to_pascal(value)));
    }
    code.close("}");
}
