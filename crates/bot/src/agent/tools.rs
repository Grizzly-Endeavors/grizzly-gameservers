//! Surface-neutral tool-parameter primitives shared by both Gary surfaces
//! (Discord and in-game chat). Each surface exposes its own *tool set* with its
//! own descriptions — the in-game surface is a deliberate read-only subset — but
//! the lookup tools they share take identical arguments. Keeping the shared
//! parameter type and the schema builders here means the two surfaces can't drift
//! into subtly different contracts for the same tool.

use schemars::{JsonSchema, SchemaGenerator};
use serde::Deserialize;

/// Arguments for a tool that targets one server by name. This is the whole
/// parameter list for the lookup tools both surfaces share, and the `name` field
/// the richer Discord tools also carry (their scope gate reads just that field off
/// any tool's arguments).
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct NameParams {
    /// Exact server name, as shown by `list_servers`.
    pub(crate) name: String,
}

/// The parameter schema for a tool that takes no arguments.
pub(crate) fn no_args_schema() -> serde_json::Value {
    serde_json::json!({ "type": "object", "properties": {} })
}

/// JSON Schema for a tool's parameters, trimmed of the metadata keys some
/// providers reject (`$schema`, `title`).
pub(crate) fn params_schema<T: JsonSchema>() -> serde_json::Value {
    let mut value = SchemaGenerator::default()
        .into_root_schema_for::<T>()
        .to_value();
    if let Some(object) = value.as_object_mut() {
        object.remove("$schema");
        object.remove("title");
    }
    value
}

#[cfg(test)]
#[path = "tests/tools.rs"]
mod tests;
