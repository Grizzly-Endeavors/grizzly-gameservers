//! The validated in-memory model of a prompt tree.
//!
//! This is the output of [`crate::load`] and the input later phases turn into
//! generated code. Everything here has already passed validation: cross-file
//! references are resolved, wire names are computed, and bodies are tokenized
//! into literal/placeholder segments ready to interleave with field values.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Every prompt file in a directory, keyed by id in sorted order for
/// deterministic downstream codegen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptTree {
    pub files: BTreeMap<String, PromptFile>,
}

/// One validated prompt file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptFile {
    pub id: String,
    /// Path relative to the prompt-directory root, for diagnostics.
    pub path: PathBuf,
    pub kind: PromptKind,
    pub annotations: Annotations,
}

/// The three file types, each carrying only what its type permits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    /// Model-visible prose with typed placeholders.
    Prompt {
        body: Vec<BodySegment>,
        /// Declared variables in body first-appearance order.
        variables: Vec<Variable>,
    },
    /// A tool definition: a static description body plus a parameter schema.
    Tool {
        /// The wire name sent to the model (explicit `name`, else `snake_case(id)`).
        wire_name: String,
        /// Whether `wire_name` came from an explicit `name` field.
        name_explicit: bool,
        /// The tool description sent to the model. Static (no placeholders).
        description: String,
        schema: ToolSchemaRef,
    },
    /// A shared parameter shape referenced by one or more tools. Its body is
    /// unused — a params file exists only to define a struct.
    Params { schema: ToolSchema },
}

/// Where a tool's parameter schema comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolSchemaRef {
    /// Defined inline on the tool.
    Inline(ToolSchema),
    /// The id of a `type: params` file whose struct this tool shares.
    Shared(String),
}

/// An ordered parameter list. Order follows the YAML, which downstream codegen
/// preserves so struct field order stays stable across builds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToolSchema {
    pub params: Vec<(String, Param)>,
}

/// One parameter definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub ty: ParamType,
    pub description: String,
    pub optional: bool,
}

/// The closed parameter type vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamType {
    String,
    Integer,
    Number,
    Boolean,
    /// A closed set of `snake_case` string values, surfaced as a generated enum.
    Enum {
        values: Vec<String>,
    },
}

/// A run of body text: either a literal segment or a `{{placeholder}}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodySegment {
    Literal(String),
    Placeholder(String),
}

/// One `annotations.variables` entry, enriched with its placeholder name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable {
    pub name: String,
    pub source: String,
    pub contents: String,
}

/// The human-facing annotations block. Never sent to the model.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Annotations {
    pub sent_when: Option<String>,
    pub used_by: Vec<UsedBy>,
    pub reasoning: Vec<String>,
}

/// One call site: a source file and the function within it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsedBy {
    pub file: String,
    pub function: String,
}
