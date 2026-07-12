//! Per-file parsing: split frontmatter from body, deserialize the frontmatter,
//! normalize and check the body, tokenize placeholders. No cross-file logic and
//! no semantic validation beyond what a single file can decide on its own —
//! that lives in [`crate::validate`].

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_yaml_ng::Mapping;

use crate::error::PromptError;
use crate::model::BodySegment;

/// A single file's raw frontmatter fields plus its normalized body. Fields are
/// `Option` so the validator can report a precise rule (missing id vs bad id)
/// rather than an opaque deserialization failure.
#[derive(Debug)]
pub(crate) struct RawFile {
    pub(crate) rel_path: PathBuf,
    pub(crate) id: Option<String>,
    pub(crate) kind: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) tool_schema: Option<Mapping>,
    pub(crate) params_from: Option<String>,
    pub(crate) annotations: Option<RawAnnotations>,
    /// The body with a single trailing newline trimmed. Not yet tokenized.
    pub(crate) body: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFrontmatter {
    id: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    name: Option<String>,
    tool_schema: Option<Mapping>,
    params_from: Option<String>,
    annotations: Option<RawAnnotations>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawAnnotations {
    pub(crate) sent_when: Option<String>,
    pub(crate) used_by: Option<Vec<RawUsedBy>>,
    pub(crate) variables: Option<std::collections::BTreeMap<String, RawVariable>>,
    pub(crate) reasoning: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawUsedBy {
    pub(crate) file: Option<String>,
    pub(crate) function: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawVariable {
    pub(crate) source: Option<String>,
    pub(crate) contents: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawParam {
    #[serde(rename = "type")]
    pub(crate) ty: Option<String>,
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) optional: bool,
    pub(crate) values: Option<Vec<String>>,
}

/// Parse one file's text into its raw frontmatter and normalized body.
///
/// # Errors
///
/// Returns [`PromptError::Frontmatter`] if the delimiters are missing or the
/// YAML does not parse, or [`PromptError::BodyWhitespace`] if the body begins or
/// ends with whitespace.
pub(crate) fn parse_file(rel_path: &Path, content: &str) -> Result<RawFile, PromptError> {
    let (yaml, raw_body) = split_frontmatter(content).ok_or_else(|| PromptError::Frontmatter {
        path: rel_path.to_path_buf(),
        message: "missing '---' frontmatter delimiters".to_owned(),
    })?;

    let front: RawFrontmatter =
        serde_yaml_ng::from_str(yaml).map_err(|err| PromptError::Frontmatter {
            path: rel_path.to_path_buf(),
            message: err.to_string(),
        })?;

    let body = normalize_body(raw_body);
    if body.starts_with(char::is_whitespace) || body.ends_with(char::is_whitespace) {
        return Err(PromptError::BodyWhitespace {
            path: rel_path.to_path_buf(),
        });
    }

    Ok(RawFile {
        rel_path: rel_path.to_path_buf(),
        id: front.id,
        kind: front.kind,
        name: front.name,
        tool_schema: front.tool_schema,
        params_from: front.params_from,
        annotations: front.annotations,
        body,
    })
}

/// Split `---`-delimited frontmatter from the body. Returns the YAML text and
/// the raw (un-trimmed) body, or `None` if the opening or closing delimiter is
/// absent.
fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let rest = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))?;
    if let Some(split) = rest.split_once("\n---\n") {
        return Some(split);
    }
    if let Some(split) = rest.split_once("\n---\r\n") {
        return Some(split);
    }
    rest.strip_suffix("\n---").map(|yaml| (yaml, ""))
}

/// Trim exactly one trailing newline (`\r\n` or `\n`), per the format rule.
fn normalize_body(body: &str) -> String {
    body.strip_suffix("\r\n")
        .or_else(|| body.strip_suffix('\n'))
        .unwrap_or(body)
        .to_owned()
}

/// Tokenize a body into literal and `{{placeholder}}` segments.
///
/// # Errors
///
/// Returns [`TokenizeError::Unterminated`] if a `{{` has no closing `}}`.
pub(crate) fn tokenize_body(body: &str) -> Result<Vec<BodySegment>, TokenizeError> {
    let mut segments = Vec::new();
    let mut literal = String::new();
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' && chars.peek() == Some(&'{') {
            chars.next();
            if !literal.is_empty() {
                segments.push(BodySegment::Literal(std::mem::take(&mut literal)));
            }
            segments.push(BodySegment::Placeholder(read_placeholder(&mut chars)?));
        } else {
            literal.push(c);
        }
    }
    if !literal.is_empty() {
        segments.push(BodySegment::Literal(literal));
    }
    Ok(segments)
}

/// Read placeholder characters up to and including the closing `}}`.
fn read_placeholder(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<String, TokenizeError> {
    let mut name = String::new();
    loop {
        match chars.next() {
            Some('}') if chars.peek() == Some(&'}') => {
                chars.next();
                return Ok(name);
            }
            Some(ch) => name.push(ch),
            None => return Err(TokenizeError::Unterminated),
        }
    }
}

/// Why body tokenization failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TokenizeError {
    Unterminated,
}

#[cfg(test)]
#[path = "tests/parse.rs"]
mod tests;
