//! Low-level helpers for turning model values into Rust source tokens.

use super::super::ident::snake_to_pascal;
use super::super::model::{Param, ParamType};

/// Render `s` as a valid Rust string literal — quoted and escaped. `{:?}` on a
/// `&str` produces exactly that (escaping `"`, `\`, control chars, and leaving
/// braces and apostrophes untouched), so a prompt body containing quotes or
/// braces embeds safely and never needs a raw string.
pub(super) fn str_lit(s: &str) -> String {
    format!("{s:?}")
}

/// Render `c` as a valid Rust char literal — quoted and escaped. Used for a
/// single-character body segment so the generated `render` pushes a `char`
/// rather than a one-char `&str` (which trips clippy's `single_char_add_str`).
pub(super) fn char_lit(c: char) -> String {
    format!("'{}'", c.escape_default())
}

/// The Rust field type for a parameter, wrapping optional parameters in
/// `Option<T>`. Enums map to the generated `<OwnerId><ParamPascal>` type.
pub(super) fn field_type(param: &Param, owner_id: &str, param_name: &str) -> String {
    let base = match &param.ty {
        ParamType::String => "String".to_owned(),
        ParamType::Integer => "i64".to_owned(),
        ParamType::Number => "f64".to_owned(),
        ParamType::Boolean => "bool".to_owned(),
        ParamType::Enum { .. } => enum_type_name(owner_id, param_name),
    };
    if param.optional {
        format!("Option<{base}>")
    } else {
        base
    }
}

/// The generated enum type name for an `enum` parameter: the owning file's id
/// concatenated with the `PascalCase` of the parameter name (design §"Generated
/// enums").
pub(super) fn enum_type_name(owner_id: &str, param_name: &str) -> String {
    format!("{owner_id}{}", snake_to_pascal(param_name))
}

/// The JSON Schema `type` string for a parameter. `enum` parameters are strings
/// with an `enum` values array (added by the schema emitter).
pub(super) fn json_type(ty: &ParamType) -> &'static str {
    match ty {
        ParamType::String | ParamType::Enum { .. } => "string",
        ParamType::Integer => "integer",
        ParamType::Number => "number",
        ParamType::Boolean => "boolean",
    }
}
