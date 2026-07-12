//! The validation orchestrator: walk a prompt directory, parse each file, run
//! every per-file and cross-file rule, and assemble the validated [`PromptTree`].
//! Each rule maps to a [`PromptError`] variant naming the file and the violation.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_yaml_ng::Mapping;

use crate::error::PromptError;
use crate::ident::{
    is_valid_enum_value, is_valid_id, is_valid_placeholder_name, pascal_to_snake, snake_to_pascal,
};
use crate::model::{
    Annotations, BodySegment, Param, ParamType, PromptFile, PromptKind, PromptTree, ToolSchema,
    ToolSchemaRef, UsedBy, Variable,
};
use crate::parse::{RawAnnotations, RawFile, RawParam, TokenizeError, parse_file, tokenize_body};

/// Whether `sent_when` is required or forbidden for a file's type.
enum SentWhen {
    Required,
    Forbidden,
}

/// Parse and validate every prompt file under `dir`, returning the validated
/// tree or the first violation encountered.
///
/// # Errors
///
/// Returns a [`PromptError`] naming the offending file and the broken rule: an
/// IO failure, a structural or annotation violation in a single file, or a
/// cross-file conflict (duplicate id, dangling `params_from`, unreferenced
/// params file, wire-name or generated-item-name collision).
pub fn load(dir: &Path) -> Result<PromptTree, PromptError> {
    let paths = collect_md_files(dir)?;
    let mut files: Vec<PromptFile> = Vec::new();
    let mut by_id: BTreeMap<String, PathBuf> = BTreeMap::new();

    for abs in &paths {
        let content = fs::read_to_string(abs).map_err(|err| PromptError::Io {
            path: abs.clone(),
            message: err.to_string(),
        })?;
        let rel = abs.strip_prefix(dir).unwrap_or(abs).to_path_buf();
        let raw = parse_file(&rel, &content)?;
        let file = validate_file(raw)?;
        if let Some(first) = by_id.get(&file.id) {
            return Err(PromptError::DuplicateId {
                id: file.id.clone(),
                first: first.clone(),
                second: file.path.clone(),
            });
        }
        by_id.insert(file.id.clone(), file.path.clone());
        files.push(file);
    }

    check_params_refs(&files)?;
    check_unreferenced_params(&files)?;
    check_wire_names(&files)?;
    check_derived_names(&files)?;

    let mut tree = BTreeMap::new();
    for file in files {
        tree.insert(file.id.clone(), file);
    }
    Ok(PromptTree { files: tree })
}

/// Recursively collect every `*.md` path under `dir`, sorted for deterministic
/// diagnostics.
fn collect_md_files(dir: &Path) -> Result<Vec<PathBuf>, PromptError> {
    let mut out = Vec::new();
    walk(dir, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), PromptError> {
    let entries = fs::read_dir(dir).map_err(|err| PromptError::Io {
        path: dir.to_path_buf(),
        message: err.to_string(),
    })?;
    for entry in entries {
        let entry = entry.map_err(|err| PromptError::Io {
            path: dir.to_path_buf(),
            message: err.to_string(),
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|err| PromptError::Io {
            path: path.clone(),
            message: err.to_string(),
        })?;
        if file_type.is_dir() {
            walk(&path, out)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
            out.push(path);
        }
    }
    Ok(())
}

/// Validate one parsed file into its model form.
fn validate_file(raw: RawFile) -> Result<PromptFile, PromptError> {
    let id = validate_id(&raw)?;
    let Some(kind_str) = raw.kind.clone() else {
        return Err(PromptError::MissingType {
            path: raw.rel_path.clone(),
        });
    };
    let (kind, annotations) = match kind_str.as_str() {
        "prompt" => validate_prompt(&raw, &id)?,
        "tool" => validate_tool(&raw, &id)?,
        "params" => validate_params(&raw)?,
        other => {
            return Err(PromptError::UnknownType {
                path: raw.rel_path.clone(),
                found: other.to_owned(),
            });
        }
    };
    Ok(PromptFile {
        id,
        path: raw.rel_path,
        kind,
        annotations,
    })
}

/// Rule: `id` present, matches the `PascalCase` grammar, equals the filename stem.
fn validate_id(raw: &RawFile) -> Result<String, PromptError> {
    let id = raw.id.clone().ok_or_else(|| PromptError::MissingId {
        path: raw.rel_path.clone(),
    })?;
    if !is_valid_id(&id) {
        return Err(PromptError::IdGrammar {
            path: raw.rel_path.clone(),
            id,
        });
    }
    let stem = raw
        .rel_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if stem != id {
        return Err(PromptError::IdFilenameMismatch {
            path: raw.rel_path.clone(),
            id,
            stem: stem.to_owned(),
        });
    }
    Ok(id)
}

fn validate_prompt(raw: &RawFile, _id: &str) -> Result<(PromptKind, Annotations), PromptError> {
    forbid_key(raw, raw.name.is_some(), "name", "prompt")?;
    forbid_key(raw, raw.tool_schema.is_some(), "tool_schema", "prompt")?;
    forbid_key(raw, raw.params_from.is_some(), "params_from", "prompt")?;

    let body = tokenize_body(&raw.body).map_err(|err| match err {
        TokenizeError::Unterminated => PromptError::UnterminatedPlaceholder {
            path: raw.rel_path.clone(),
        },
    })?;
    let placeholders = unique_placeholders(&body);
    for name in &placeholders {
        if !is_valid_placeholder_name(name) {
            return Err(PromptError::PlaceholderName {
                path: raw.rel_path.clone(),
                name: name.clone(),
            });
        }
    }

    let annotations = validate_common_annotations(raw, &SentWhen::Required)?;
    let variables = validate_variables(raw, &placeholders)?;
    Ok((PromptKind::Prompt { body, variables }, annotations))
}

fn validate_tool(raw: &RawFile, id: &str) -> Result<(PromptKind, Annotations), PromptError> {
    forbid_annotation_variables(raw)?;
    reject_static_placeholder(raw, &raw.body, "body")?;
    let schema = validate_tool_schema_source(raw)?;
    let wire_name = raw.name.clone().unwrap_or_else(|| pascal_to_snake(id));
    let name_explicit = raw.name.is_some();
    let annotations = validate_common_annotations(raw, &SentWhen::Required)?;
    Ok((
        PromptKind::Tool {
            wire_name,
            name_explicit,
            description: raw.body.clone(),
            schema,
        },
        annotations,
    ))
}

fn validate_params(raw: &RawFile) -> Result<(PromptKind, Annotations), PromptError> {
    forbid_key(raw, raw.name.is_some(), "name", "params")?;
    forbid_key(raw, raw.params_from.is_some(), "params_from", "params")?;
    forbid_annotation_variables(raw)?;
    reject_static_placeholder(raw, &raw.body, "body")?;
    let map = raw
        .tool_schema
        .as_ref()
        .ok_or_else(|| PromptError::ParamsSchemaMissing {
            path: raw.rel_path.clone(),
        })?;
    let schema = build_tool_schema(raw, map)?;
    let annotations = validate_common_annotations(raw, &SentWhen::Forbidden)?;
    Ok((PromptKind::Params { schema }, annotations))
}

/// Rule: a tool declares exactly one of `tool_schema` or `params_from`.
fn validate_tool_schema_source(raw: &RawFile) -> Result<ToolSchemaRef, PromptError> {
    match (raw.tool_schema.as_ref(), raw.params_from.as_ref()) {
        (Some(_), Some(_)) => Err(PromptError::SchemaSourceAmbiguous {
            path: raw.rel_path.clone(),
        }),
        (None, None) => Err(PromptError::SchemaSourceMissing {
            path: raw.rel_path.clone(),
        }),
        (Some(map), None) => Ok(ToolSchemaRef::Inline(build_tool_schema(raw, map)?)),
        (None, Some(target)) => Ok(ToolSchemaRef::Shared(target.clone())),
    }
}

/// Build and validate a `tool_schema` map into an ordered [`ToolSchema`].
fn build_tool_schema(raw: &RawFile, map: &Mapping) -> Result<ToolSchema, PromptError> {
    let mut params = Vec::new();
    for (key, value) in map {
        let name = key.as_str().ok_or_else(|| PromptError::Frontmatter {
            path: raw.rel_path.clone(),
            message: "tool_schema keys must be strings".to_owned(),
        })?;
        let raw_param: RawParam =
            serde_yaml_ng::from_value(value.clone()).map_err(|err| PromptError::Frontmatter {
                path: raw.rel_path.clone(),
                message: format!("parameter '{name}': {err}"),
            })?;
        let param = validate_param(raw, name, &raw_param)?;
        params.push((name.to_owned(), param));
    }
    Ok(ToolSchema { params })
}

/// Rule set for a single parameter: legal name, known type, present description,
/// and enum-value grammar.
fn validate_param(raw: &RawFile, name: &str, raw_param: &RawParam) -> Result<Param, PromptError> {
    if !is_valid_placeholder_name(name) {
        return Err(PromptError::PlaceholderName {
            path: raw.rel_path.clone(),
            name: name.to_owned(),
        });
    }
    let Some(ty_str) = raw_param.ty.as_deref() else {
        return Err(PromptError::ParamType {
            path: raw.rel_path.clone(),
            param: name.to_owned(),
            found: "(none)".to_owned(),
        });
    };
    let description = non_empty(raw_param.description.as_deref()).ok_or_else(|| {
        PromptError::ParamDescription {
            path: raw.rel_path.clone(),
            param: name.to_owned(),
        }
    })?;
    reject_static_placeholder(
        raw,
        &description,
        &format!("parameter '{name}' description"),
    )?;
    let ty = validate_param_type(raw, name, ty_str, raw_param)?;
    Ok(Param {
        ty,
        description,
        optional: raw_param.optional,
    })
}

fn validate_param_type(
    raw: &RawFile,
    name: &str,
    ty_str: &str,
    raw_param: &RawParam,
) -> Result<ParamType, PromptError> {
    match ty_str {
        "string" => reject_values(raw, name, raw_param).map(|()| ParamType::String),
        "integer" => reject_values(raw, name, raw_param).map(|()| ParamType::Integer),
        "number" => reject_values(raw, name, raw_param).map(|()| ParamType::Number),
        "boolean" => reject_values(raw, name, raw_param).map(|()| ParamType::Boolean),
        "enum" => validate_enum_values(raw, name, raw_param),
        other => Err(PromptError::ParamType {
            path: raw.rel_path.clone(),
            param: name.to_owned(),
            found: other.to_owned(),
        }),
    }
}

fn validate_enum_values(
    raw: &RawFile,
    name: &str,
    raw_param: &RawParam,
) -> Result<ParamType, PromptError> {
    let values = raw_param
        .values
        .as_ref()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| PromptError::EnumValuesMissing {
            path: raw.rel_path.clone(),
            param: name.to_owned(),
        })?;
    for value in values {
        if !is_valid_enum_value(value) {
            return Err(PromptError::EnumValueGrammar {
                path: raw.rel_path.clone(),
                param: name.to_owned(),
                value: value.clone(),
            });
        }
    }
    Ok(ParamType::Enum {
        values: values.clone(),
    })
}

fn reject_values(raw: &RawFile, name: &str, raw_param: &RawParam) -> Result<(), PromptError> {
    if raw_param.values.is_some() {
        return Err(PromptError::ValuesOnNonEnum {
            path: raw.rel_path.clone(),
            param: name.to_owned(),
        });
    }
    Ok(())
}

/// Rule: `{{` must not appear in text that is sent verbatim (tool/params bodies
/// and parameter descriptions).
fn reject_static_placeholder(raw: &RawFile, text: &str, location: &str) -> Result<(), PromptError> {
    if text.contains("{{") {
        return Err(PromptError::PlaceholderInStaticText {
            path: raw.rel_path.clone(),
            location: location.to_owned(),
        });
    }
    Ok(())
}

fn forbid_key(raw: &RawFile, present: bool, key: &str, kind: &str) -> Result<(), PromptError> {
    if present {
        return Err(PromptError::KeyNotAllowed {
            path: raw.rel_path.clone(),
            key: key.to_owned(),
            kind: kind.to_owned(),
        });
    }
    Ok(())
}

fn forbid_annotation_variables(raw: &RawFile) -> Result<(), PromptError> {
    if raw
        .annotations
        .as_ref()
        .is_some_and(|ann| ann.variables.is_some())
    {
        return Err(PromptError::ForbiddenAnnotation {
            path: raw.rel_path.clone(),
            field: "variables".to_owned(),
        });
    }
    Ok(())
}

/// Distinct placeholder names in body first-appearance order.
fn unique_placeholders(body: &[BodySegment]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for segment in body {
        if let BodySegment::Placeholder(name) = segment
            && seen.insert(name.clone())
        {
            out.push(name.clone());
        }
    }
    out
}

/// Rule: `variables` present iff the body has placeholders, and each placeholder
/// has exactly one non-empty entry (both directions).
fn validate_variables(
    raw: &RawFile,
    placeholders: &[String],
) -> Result<Vec<Variable>, PromptError> {
    let vars = raw
        .annotations
        .as_ref()
        .and_then(|ann| ann.variables.as_ref());
    match (placeholders.is_empty(), vars) {
        (true, Some(_)) => Err(PromptError::VariablesKeyMismatch {
            path: raw.rel_path.clone(),
            detail: "'variables' is present but the body has no placeholders".to_owned(),
        }),
        (false, None) => Err(PromptError::VariablesKeyMismatch {
            path: raw.rel_path.clone(),
            detail: "the body has placeholders but no 'variables' annotation".to_owned(),
        }),
        (true, None) => Ok(Vec::new()),
        (false, Some(map)) => build_variables(raw, placeholders, map),
    }
}

fn build_variables(
    raw: &RawFile,
    placeholders: &[String],
    map: &BTreeMap<String, crate::parse::RawVariable>,
) -> Result<Vec<Variable>, PromptError> {
    let mut out = Vec::new();
    for name in placeholders {
        let entry = map
            .get(name)
            .ok_or_else(|| PromptError::UndeclaredPlaceholder {
                path: raw.rel_path.clone(),
                name: name.clone(),
            })?;
        let source =
            non_empty(entry.source.as_deref()).ok_or_else(|| PromptError::EmptyAnnotation {
                path: raw.rel_path.clone(),
                field: format!("variables.{name}.source"),
            })?;
        let contents =
            non_empty(entry.contents.as_deref()).ok_or_else(|| PromptError::EmptyAnnotation {
                path: raw.rel_path.clone(),
                field: format!("variables.{name}.contents"),
            })?;
        out.push(Variable {
            name: name.clone(),
            source,
            contents,
        });
    }
    let declared: BTreeSet<&str> = placeholders.iter().map(String::as_str).collect();
    for key in map.keys() {
        if !declared.contains(key.as_str()) {
            return Err(PromptError::UnusedVariable {
                path: raw.rel_path.clone(),
                name: key.clone(),
            });
        }
    }
    Ok(out)
}

/// Common annotation rules shared by all three types: `used_by`, `reasoning`,
/// and the type-specific `sent_when` policy.
fn validate_common_annotations(
    raw: &RawFile,
    sent_when: &SentWhen,
) -> Result<Annotations, PromptError> {
    let ann = raw
        .annotations
        .as_ref()
        .ok_or_else(|| PromptError::MissingAnnotation {
            path: raw.rel_path.clone(),
            field: "annotations".to_owned(),
        })?;
    let sent = validate_sent_when(raw, ann, sent_when)?;
    let used_by = validate_used_by(raw, ann)?;
    let reasoning = validate_reasoning(raw, ann)?;
    Ok(Annotations {
        sent_when: sent,
        used_by,
        reasoning,
    })
}

fn validate_sent_when(
    raw: &RawFile,
    ann: &RawAnnotations,
    rule: &SentWhen,
) -> Result<Option<String>, PromptError> {
    match rule {
        SentWhen::Required => {
            let value = non_empty(ann.sent_when.as_deref());
            match (ann.sent_when.is_some(), value) {
                (_, Some(text)) => Ok(Some(text)),
                (true, None) => Err(PromptError::EmptyAnnotation {
                    path: raw.rel_path.clone(),
                    field: "sent_when".to_owned(),
                }),
                (false, None) => Err(PromptError::MissingAnnotation {
                    path: raw.rel_path.clone(),
                    field: "sent_when".to_owned(),
                }),
            }
        }
        SentWhen::Forbidden => {
            if ann.sent_when.is_some() {
                return Err(PromptError::ForbiddenAnnotation {
                    path: raw.rel_path.clone(),
                    field: "sent_when".to_owned(),
                });
            }
            Ok(None)
        }
    }
}

fn validate_used_by(raw: &RawFile, ann: &RawAnnotations) -> Result<Vec<UsedBy>, PromptError> {
    let list = ann
        .used_by
        .as_ref()
        .ok_or_else(|| PromptError::MissingAnnotation {
            path: raw.rel_path.clone(),
            field: "used_by".to_owned(),
        })?;
    if list.is_empty() {
        return Err(PromptError::EmptyAnnotation {
            path: raw.rel_path.clone(),
            field: "used_by".to_owned(),
        });
    }
    let mut out = Vec::new();
    for entry in list {
        let file =
            non_empty(entry.file.as_deref()).ok_or_else(|| PromptError::EmptyAnnotation {
                path: raw.rel_path.clone(),
                field: "used_by.file".to_owned(),
            })?;
        let function =
            non_empty(entry.function.as_deref()).ok_or_else(|| PromptError::EmptyAnnotation {
                path: raw.rel_path.clone(),
                field: "used_by.function".to_owned(),
            })?;
        out.push(UsedBy { file, function });
    }
    Ok(out)
}

fn validate_reasoning(raw: &RawFile, ann: &RawAnnotations) -> Result<Vec<String>, PromptError> {
    let list = ann
        .reasoning
        .as_ref()
        .ok_or_else(|| PromptError::MissingAnnotation {
            path: raw.rel_path.clone(),
            field: "reasoning".to_owned(),
        })?;
    if list.is_empty() {
        return Err(PromptError::EmptyAnnotation {
            path: raw.rel_path.clone(),
            field: "reasoning".to_owned(),
        });
    }
    for note in list {
        if note.trim().is_empty() {
            return Err(PromptError::EmptyAnnotation {
                path: raw.rel_path.clone(),
                field: "reasoning".to_owned(),
            });
        }
    }
    Ok(list.clone())
}

/// Rule: every `params_from` names an existing params file.
fn check_params_refs(files: &[PromptFile]) -> Result<(), PromptError> {
    let params_ids = params_id_set(files);
    for file in files {
        if let PromptKind::Tool {
            schema: ToolSchemaRef::Shared(target),
            ..
        } = &file.kind
            && !params_ids.contains(target.as_str())
        {
            return Err(PromptError::UnknownParamsRef {
                path: file.path.clone(),
                params_from: target.clone(),
            });
        }
    }
    Ok(())
}

/// Rule: every params file is referenced by at least one tool.
fn check_unreferenced_params(files: &[PromptFile]) -> Result<(), PromptError> {
    let referenced: BTreeSet<&str> = files
        .iter()
        .filter_map(|file| match &file.kind {
            PromptKind::Tool {
                schema: ToolSchemaRef::Shared(id),
                ..
            } => Some(id.as_str()),
            PromptKind::Tool {
                schema: ToolSchemaRef::Inline(_),
                ..
            }
            | PromptKind::Prompt { .. }
            | PromptKind::Params { .. } => None,
        })
        .collect();
    for file in files {
        if let PromptKind::Params { .. } = &file.kind
            && !referenced.contains(file.id.as_str())
        {
            return Err(PromptError::UnreferencedParams {
                path: file.path.clone(),
                id: file.id.clone(),
            });
        }
    }
    Ok(())
}

/// Rule: colliding wire names must be explicitly declared on every side.
fn check_wire_names(files: &[PromptFile]) -> Result<(), PromptError> {
    let mut groups: BTreeMap<String, Vec<(PathBuf, bool)>> = BTreeMap::new();
    for file in files {
        if let PromptKind::Tool {
            wire_name,
            name_explicit,
            ..
        } = &file.kind
        {
            groups
                .entry(wire_name.clone())
                .or_default()
                .push((file.path.clone(), *name_explicit));
        }
    }
    for (name, members) in groups {
        if members.len() > 1 && !members.iter().all(|(_, explicit)| *explicit) {
            let paths = members.into_iter().map(|(path, _)| path).collect();
            return Err(PromptError::WireNameCollision { name, files: paths });
        }
    }
    Ok(())
}

/// Rule: no generated Rust item name (a synthesized `<Id>Params` struct or an
/// `<Id><Param>` enum) collides with a declared id or another generated name.
fn check_derived_names(files: &[PromptFile]) -> Result<(), PromptError> {
    let mut names: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for file in files {
        names
            .entry(file.id.clone())
            .or_default()
            .push(file.path.clone());
    }
    for file in files {
        register_derived_names(&mut names, file);
    }
    for (name, sources) in names {
        if sources.len() > 1 {
            return Err(PromptError::DerivedNameCollision { name, sources });
        }
    }
    Ok(())
}

fn register_derived_names(names: &mut BTreeMap<String, Vec<PathBuf>>, file: &PromptFile) {
    match &file.kind {
        PromptKind::Tool {
            schema: ToolSchemaRef::Inline(schema),
            ..
        } => {
            if !schema.params.is_empty() {
                names
                    .entry(format!("{}Params", file.id))
                    .or_default()
                    .push(file.path.clone());
            }
            register_enum_names(names, &file.id, schema, &file.path);
        }
        PromptKind::Params { schema } => register_enum_names(names, &file.id, schema, &file.path),
        PromptKind::Tool {
            schema: ToolSchemaRef::Shared(_),
            ..
        }
        | PromptKind::Prompt { .. } => {}
    }
}

fn register_enum_names(
    names: &mut BTreeMap<String, Vec<PathBuf>>,
    owner_id: &str,
    schema: &ToolSchema,
    path: &Path,
) {
    for (param_name, param) in &schema.params {
        if let ParamType::Enum { .. } = &param.ty {
            let enum_name = format!("{owner_id}{}", snake_to_pascal(param_name));
            names.entry(enum_name).or_default().push(path.to_path_buf());
        }
    }
}

/// A trimmed, non-empty clone of an optional annotation string, or `None`.
fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .filter(|s| !s.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn params_id_set(files: &[PromptFile]) -> BTreeSet<&str> {
    files
        .iter()
        .filter_map(|file| match &file.kind {
            PromptKind::Params { .. } => Some(file.id.as_str()),
            PromptKind::Prompt { .. } | PromptKind::Tool { .. } => None,
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/validate.rs"]
mod tests;
