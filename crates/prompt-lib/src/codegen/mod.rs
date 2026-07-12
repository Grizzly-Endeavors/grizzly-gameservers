//! The codegen face: parse and validate a prompt directory, then emit the
//! generated `prompts.rs` module.
//!
//! Everything under here is gated behind the `codegen` feature (declared once,
//! on the `mod codegen` line in `lib.rs`), so the runtime face pulls in neither
//! the YAML parser nor `serde`. [`load`] produces the validated [`PromptTree`];
//! [`generate`]/[`emit`] turn that tree into Rust source.

mod emit;
mod error;
mod ident;
mod model;
mod parse;
mod validate;

pub use emit::{emit, generate};
pub use error::PromptError;
pub use model::{
    Annotations, BodySegment, Param, ParamType, PromptFile, PromptKind, PromptTree, ToolSchema,
    ToolSchemaRef, UsedBy, Variable,
};
pub use validate::load;
