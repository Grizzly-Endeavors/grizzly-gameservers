//! Typed validation errors. Every variant names the file it came from and the
//! rule it broke, so a failing build points straight at the fix. Follows the
//! workspace convention of a hand-rolled enum with a manual [`Display`] (as in
//! the supervisor's `FsError`) rather than pulling in `thiserror`.

use std::fmt;
use std::path::{Path, PathBuf};

/// A single build-time validation failure.
///
/// Paths are relative to the prompt directory root where possible, so `Display`
/// reads like `gary/EditFile.md: ...`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptError {
    /// A file (or the root directory) could not be read.
    Io { path: PathBuf, message: String },
    /// The frontmatter delimiters are missing or the YAML did not parse.
    Frontmatter { path: PathBuf, message: String },
    /// No `id` field.
    MissingId { path: PathBuf },
    /// `id` does not match the `PascalCase` grammar `([A-Z][a-z0-9]*)+`.
    IdGrammar { path: PathBuf, id: String },
    /// `id` does not equal the file's stem.
    IdFilenameMismatch {
        path: PathBuf,
        id: String,
        stem: String,
    },
    /// No `type` field.
    MissingType { path: PathBuf },
    /// `type` is not one of `prompt`, `tool`, `params`.
    UnknownType { path: PathBuf, found: String },
    /// A top-level key is not permitted for this file's type.
    KeyNotAllowed {
        path: PathBuf,
        key: String,
        kind: String,
    },
    /// A tool declares both `tool_schema` and `params_from`.
    SchemaSourceAmbiguous { path: PathBuf },
    /// A tool declares neither `tool_schema` nor `params_from`.
    SchemaSourceMissing { path: PathBuf },
    /// The body begins or ends with whitespace (beyond the trimmed trailing newline).
    BodyWhitespace { path: PathBuf },
    /// A `{{` appears in text that must be static (a tool/params body or a description).
    PlaceholderInStaticText { path: PathBuf, location: String },
    /// A `{{` opened without a closing `}}`.
    UnterminatedPlaceholder { path: PathBuf },
    /// A placeholder name is not a legal, non-keyword lowercase identifier.
    PlaceholderName { path: PathBuf, name: String },
    /// A body placeholder has no matching `variables` annotation entry.
    UndeclaredPlaceholder { path: PathBuf, name: String },
    /// A `variables` annotation entry names a placeholder the body never uses.
    UnusedVariable { path: PathBuf, name: String },
    /// The `variables` key is present without placeholders, or absent with them.
    VariablesKeyMismatch { path: PathBuf, detail: String },
    /// A parameter's `type` is unknown or an unsupported (array/object) shape.
    ParamType {
        path: PathBuf,
        param: String,
        found: String,
    },
    /// A parameter has no `description`.
    ParamDescription { path: PathBuf, param: String },
    /// A params file declares no `tool_schema`.
    ParamsSchemaMissing { path: PathBuf },
    /// An `enum` parameter has no `values` list.
    EnumValuesMissing { path: PathBuf, param: String },
    /// An enum value does not match the grammar `[a-z][a-z0-9_]*`.
    EnumValueGrammar {
        path: PathBuf,
        param: String,
        value: String,
    },
    /// A non-`enum` parameter carries a `values` list.
    ValuesOnNonEnum { path: PathBuf, param: String },
    /// A required annotation field is absent.
    MissingAnnotation { path: PathBuf, field: String },
    /// An annotation field is present but blank (recursively).
    EmptyAnnotation { path: PathBuf, field: String },
    /// An annotation field is present but not permitted for this file's type.
    ForbiddenAnnotation { path: PathBuf, field: String },
    /// Two files declare the same id.
    DuplicateId {
        id: String,
        first: PathBuf,
        second: PathBuf,
    },
    /// A tool's `params_from` names no existing params file.
    UnknownParamsRef { path: PathBuf, params_from: String },
    /// A params file is referenced by no tool.
    UnreferencedParams { path: PathBuf, id: String },
    /// Two tools resolve to the same wire name without both declaring it explicitly.
    WireNameCollision { name: String, files: Vec<PathBuf> },
    /// A synthesized Rust item name (params struct or enum) collides.
    DerivedNameCollision { name: String, sources: Vec<PathBuf> },
}

impl fmt::Display for PromptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(path) = self.location() {
            write!(f, "{}: ", path.display())?;
        }
        self.write_reason(f)
    }
}

impl PromptError {
    /// The single file this error anchors to, printed as a prefix. Errors that
    /// span multiple files (collisions) or none report their paths inline in
    /// [`Self::write_reason`] and return `None` here.
    fn location(&self) -> Option<&Path> {
        match self {
            Self::Io { path, .. }
            | Self::Frontmatter { path, .. }
            | Self::MissingId { path }
            | Self::IdGrammar { path, .. }
            | Self::IdFilenameMismatch { path, .. }
            | Self::MissingType { path }
            | Self::UnknownType { path, .. }
            | Self::KeyNotAllowed { path, .. }
            | Self::SchemaSourceAmbiguous { path }
            | Self::SchemaSourceMissing { path }
            | Self::BodyWhitespace { path }
            | Self::PlaceholderInStaticText { path, .. }
            | Self::UnterminatedPlaceholder { path }
            | Self::PlaceholderName { path, .. }
            | Self::UndeclaredPlaceholder { path, .. }
            | Self::UnusedVariable { path, .. }
            | Self::VariablesKeyMismatch { path, .. }
            | Self::ParamType { path, .. }
            | Self::ParamDescription { path, .. }
            | Self::ParamsSchemaMissing { path }
            | Self::EnumValuesMissing { path, .. }
            | Self::EnumValueGrammar { path, .. }
            | Self::ValuesOnNonEnum { path, .. }
            | Self::MissingAnnotation { path, .. }
            | Self::EmptyAnnotation { path, .. }
            | Self::ForbiddenAnnotation { path, .. }
            | Self::UnknownParamsRef { path, .. }
            | Self::UnreferencedParams { path, .. } => Some(path),
            Self::DuplicateId { .. }
            | Self::WireNameCollision { .. }
            | Self::DerivedNameCollision { .. } => None,
        }
    }

    /// The rule-specific reason, without the file prefix.
    fn write_reason(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { message, .. } => write!(f, "{message}"),
            Self::Frontmatter { message, .. } => write!(f, "invalid frontmatter: {message}"),
            Self::MissingId { .. } => write!(f, "missing 'id'"),
            Self::IdGrammar { id, .. } => write!(f, "id '{id}' is not PascalCase"),
            Self::IdFilenameMismatch { id, stem, .. } => {
                write!(f, "id '{id}' does not match filename stem '{stem}'")
            }
            Self::MissingType { .. } => write!(f, "missing 'type'"),
            Self::UnknownType { found, .. } => {
                write!(f, "unknown type '{found}', expected prompt/tool/params")
            }
            Self::KeyNotAllowed { key, kind, .. } => {
                write!(f, "key '{key}' is not allowed on a '{kind}' file")
            }
            Self::SchemaSourceAmbiguous { .. } => {
                write!(f, "declares both 'tool_schema' and 'params_from'")
            }
            Self::SchemaSourceMissing { .. } => {
                write!(f, "declares neither 'tool_schema' nor 'params_from'")
            }
            Self::BodyWhitespace { .. } => write!(f, "body must not begin or end with whitespace"),
            Self::PlaceholderInStaticText { location, .. } => {
                write!(f, "'{{{{' is not allowed in {location}")
            }
            Self::UnterminatedPlaceholder { .. } => write!(f, "'{{{{' without a closing '}}}}'"),
            Self::PlaceholderName { name, .. } => {
                write!(f, "'{name}' is not a legal non-keyword identifier")
            }
            Self::UndeclaredPlaceholder { name, .. } => {
                write!(f, "placeholder {{{{{name}}}}} has no variables entry")
            }
            Self::UnusedVariable { name, .. } => {
                write!(f, "variables entry '{name}' matches no placeholder")
            }
            Self::VariablesKeyMismatch { detail, .. } => write!(f, "{detail}"),
            Self::ParamType { param, found, .. } => {
                write!(f, "parameter '{param}' has unsupported type '{found}'")
            }
            Self::ParamDescription { param, .. } => {
                write!(f, "parameter '{param}' has no description")
            }
            Self::ParamsSchemaMissing { .. } => {
                write!(f, "a params file must declare 'tool_schema'")
            }
            Self::EnumValuesMissing { param, .. } => {
                write!(f, "enum parameter '{param}' has no 'values' list")
            }
            Self::EnumValueGrammar { param, value, .. } => {
                write!(f, "enum '{param}' value '{value}' is not snake_case")
            }
            Self::ValuesOnNonEnum { param, .. } => {
                write!(f, "parameter '{param}' has 'values' but is not an enum")
            }
            Self::MissingAnnotation { field, .. } => {
                write!(f, "missing required annotation '{field}'")
            }
            Self::EmptyAnnotation { field, .. } => {
                write!(f, "annotation '{field}' must not be empty")
            }
            Self::ForbiddenAnnotation { field, .. } => {
                write!(
                    f,
                    "annotation '{field}' is not permitted on this file's type"
                )
            }
            Self::DuplicateId { id, first, second } => {
                write!(
                    f,
                    "duplicate id '{id}': {} and {}",
                    first.display(),
                    second.display()
                )
            }
            Self::UnknownParamsRef { params_from, .. } => {
                write!(
                    f,
                    "params_from '{params_from}' names no existing params file"
                )
            }
            Self::UnreferencedParams { id, .. } => {
                write!(f, "params file '{id}' is referenced by no tool")
            }
            Self::WireNameCollision { name, files } => {
                write!(
                    f,
                    "wire name '{name}' collides across {}",
                    join_paths(files)
                )
            }
            Self::DerivedNameCollision { name, sources } => {
                write!(
                    f,
                    "generated item name '{name}' collides across {}",
                    join_paths(sources)
                )
            }
        }
    }
}

impl std::error::Error for PromptError {}

fn join_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}
