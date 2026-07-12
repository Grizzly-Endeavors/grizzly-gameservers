//! Compile prompt files into typed Rust accessors.
//!
//! A prompt file is Markdown with YAML frontmatter: the frontmatter declares an
//! id, a type (`prompt`, `tool`, or `params`), and human-facing annotations; the
//! body is the verbatim text sent to the model. This crate has three
//! dependency-isolated faces, selected by cargo feature:
//!
//! - **runtime** (default): only [`ToolSpec`], the shared type generated code
//!   references. No YAML parser, no file I/O.
//! - **codegen** (feature `codegen`, a build-dependency): [`load`] parses and
//!   validates a prompt directory into a [`PromptTree`], and [`emit`]/[`generate`]
//!   turn that tree into the generated `prompts.rs` module.
//! - **verify** (feature `verify`, a dev-dependency): cross-references annotations
//!   against the source tree. Added in a later phase.

// Generated code (and this crate's own golden self-test) refer to types by an
// absolute `grizzly_prompt_lib::…` path. This alias makes those paths resolve
// inside the crate exactly as they do in a downstream consumer.
extern crate self as grizzly_prompt_lib;

mod spec;

#[doc(hidden)]
pub use spec::__private;
pub use spec::ToolSpec;

#[cfg(feature = "codegen")]
mod codegen;
#[cfg(feature = "codegen")]
pub use codegen::{
    Annotations, BodySegment, Param, ParamType, PromptError, PromptFile, PromptKind, PromptTree,
    ToolSchema, ToolSchemaRef, UsedBy, Variable, emit, generate, load,
};
