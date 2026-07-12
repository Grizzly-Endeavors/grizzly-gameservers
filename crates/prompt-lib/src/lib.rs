//! Compile prompt files into typed Rust accessors.
//!
//! A prompt file is Markdown with YAML frontmatter: the frontmatter declares an
//! id, a type (`prompt`, `tool`, or `params`), and human-facing annotations; the
//! body is the verbatim text sent to the model. This crate's job in phase one is
//! the front half of that pipeline — parse a directory of prompt files and run
//! every structural validation rule, producing a validated in-memory model that
//! later phases turn into generated code.
//!
//! [`load`] is the entry point: give it a prompt directory and it returns the
//! validated [`PromptTree`] or a [`PromptError`] naming the offending file and
//! the rule it broke.

mod error;
mod ident;
mod model;
mod parse;
mod validate;

pub use error::PromptError;
pub use model::{
    Annotations, BodySegment, Param, ParamType, PromptFile, PromptKind, PromptTree, ToolSchema,
    ToolSchemaRef, UsedBy, Variable,
};
pub use validate::load;
