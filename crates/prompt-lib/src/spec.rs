//! The runtime face: the minimal shared type generated code references.
//!
//! This is the whole of the default (runtime) dependency face — no YAML parser,
//! no loader, no template engine, no file I/O. A consuming crate's generated
//! `spec()` methods return a [`ToolSpec`]; the consumer's own glue converts it
//! into its LLM client's wire type (design §"External touchpoints").

/// A fully-assembled tool specification: the wire name and description are
/// compiled-in statics, and the JSON parameter schema is built by each
/// generated `spec()` call.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: serde_json::Value,
}

/// Re-exports that generated code references by an absolute
/// `grizzly_prompt_lib::__private::…` path, so the emitted schema `Value` is
/// built with *this* crate's `serde_json` — version-locked to
/// [`ToolSpec::parameters`], not whatever the consumer happens to depend on.
///
/// Not part of the public API; the leading underscore marks it as codegen
/// support that may change without a semver bump.
#[doc(hidden)]
pub mod __private {
    pub use serde_json;
}
