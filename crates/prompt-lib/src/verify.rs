//! The verify face: cross-reference prompt annotations against the source tree.
//!
//! [`load`] validates each prompt file in isolation, but a `used_by` entry
//! points at a call site the build step never sees. [`verify_annotations`]
//! closes that gap at test time (blocking a merge, not compilation): every
//! `used_by` entry must name a real source file whose text mentions both the
//! prompt id and the named function, and no prompt may be orphaned — its id
//! absent from the entire source tree. Matching is plain substring, textual by
//! design (no AST parsing): the id *is* the generated type name, so it appears
//! verbatim at every call site.
//!
//! This face is a dev-dependency only; it enables `codegen` to reuse [`load`].

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::{PromptError, load};

/// Cross-reference every prompt's annotations against the crate's source tree.
///
/// `prompts_dir` is the crate's `prompts/` directory; `src_dir` is its `src/`
/// directory. `used_by.file` paths are resolved relative to `src_dir`, and the
/// orphan scan reads every `*.rs` file beneath it. All violations are collected
/// and returned together so a maintainer sees the whole picture at once.
///
/// # Errors
///
/// Returns a [`VerifyReport`] if the prompt tree fails to [`load`], if a
/// `used_by` entry names a missing source file or one lacking the id or
/// function, if a prompt id appears in no source file, or if a source file
/// cannot be read.
pub fn verify_annotations(prompts_dir: &Path, src_dir: &Path) -> Result<(), VerifyReport> {
    let tree = match load(prompts_dir) {
        Ok(tree) => tree,
        Err(err) => return Err(VerifyReport(vec![VerifyError::Load(err)])),
    };

    let (sources, mut failures) = collect_sources(src_dir);

    for file in tree.files.values() {
        if !sources.values().any(|text| text.contains(&file.id)) {
            failures.push(VerifyError::OrphanedPrompt {
                prompt: file.path.clone(),
                id: file.id.clone(),
            });
        }
        for entry in &file.annotations.used_by {
            match sources.get(entry.file.as_str()) {
                None => failures.push(VerifyError::MissingSourceFile {
                    prompt: file.path.clone(),
                    id: file.id.clone(),
                    file: entry.file.clone(),
                    function: entry.function.clone(),
                }),
                Some(text) => {
                    if !text.contains(&file.id) {
                        failures.push(VerifyError::IdNotInFile {
                            prompt: file.path.clone(),
                            id: file.id.clone(),
                            file: entry.file.clone(),
                        });
                    }
                    if !text.contains(&entry.function) {
                        failures.push(VerifyError::FunctionNotInFile {
                            prompt: file.path.clone(),
                            id: file.id.clone(),
                            file: entry.file.clone(),
                            function: entry.function.clone(),
                        });
                    }
                }
            }
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(VerifyReport(failures))
    }
}

/// Read every `*.rs` file under `src_dir` into a map keyed by its `src`-relative
/// path with forward-slash separators — the form `used_by.file` is written in.
/// Any read failure becomes a [`VerifyError::SourceIo`] entry rather than
/// aborting, so one unreadable file doesn't mask real annotation drift.
fn collect_sources(src_dir: &Path) -> (BTreeMap<String, String>, Vec<VerifyError>) {
    let mut sources = BTreeMap::new();
    let mut failures = Vec::new();
    walk_sources(src_dir, src_dir, &mut sources, &mut failures);
    (sources, failures)
}

fn walk_sources(
    base: &Path,
    dir: &Path,
    sources: &mut BTreeMap<String, String>,
    failures: &mut Vec<VerifyError>,
) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            failures.push(VerifyError::SourceIo {
                path: dir.to_path_buf(),
                message: err.to_string(),
            });
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                failures.push(VerifyError::SourceIo {
                    path: dir.to_path_buf(),
                    message: err.to_string(),
                });
                continue;
            }
        };
        let path = entry.path();
        match entry.file_type() {
            Ok(file_type) if file_type.is_dir() => walk_sources(base, &path, sources, failures),
            Ok(_) if path.extension().and_then(|ext| ext.to_str()) == Some("rs") => {
                match fs::read_to_string(&path) {
                    Ok(contents) => {
                        if let Some(key) = rel_key(base, &path) {
                            sources.insert(key, contents);
                        }
                    }
                    Err(err) => failures.push(VerifyError::SourceIo {
                        path: path.clone(),
                        message: err.to_string(),
                    }),
                }
            }
            Ok(_) => {}
            Err(err) => failures.push(VerifyError::SourceIo {
                path: path.clone(),
                message: err.to_string(),
            }),
        }
    }
}

/// `path` relative to `base`, joined with `/` so the key matches a `used_by.file`
/// value regardless of the host separator.
fn rel_key(base: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(base).ok()?;
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(parts.join("/"))
}

/// One annotation cross-reference failure. Every variant names the prompt file
/// (path relative to the prompt directory) and the stale entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// The prompt tree itself failed to load or validate.
    Load(PromptError),
    /// A `used_by` entry names a file absent under `src_dir`.
    MissingSourceFile {
        prompt: PathBuf,
        id: String,
        file: String,
        function: String,
    },
    /// A `used_by` file exists but its text never mentions the prompt id.
    IdNotInFile {
        prompt: PathBuf,
        id: String,
        file: String,
    },
    /// A `used_by` file exists but its text never mentions the named function.
    FunctionNotInFile {
        prompt: PathBuf,
        id: String,
        file: String,
        function: String,
    },
    /// The prompt id appears in no source file under `src_dir`.
    OrphanedPrompt { prompt: PathBuf, id: String },
    /// A source file (or directory) could not be read during the scan.
    SourceIo { path: PathBuf, message: String },
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Load(err) => write!(f, "{err}"),
            Self::MissingSourceFile {
                prompt,
                file,
                function,
                ..
            } => write!(
                f,
                "{}: used_by names '{}' (function '{}'), which does not exist under src",
                prompt.display(),
                file,
                function
            ),
            Self::IdNotInFile { prompt, id, file } => write!(
                f,
                "{}: id '{}' does not appear in used_by file '{}'",
                prompt.display(),
                id,
                file
            ),
            Self::FunctionNotInFile {
                prompt,
                file,
                function,
                ..
            } => write!(
                f,
                "{}: function '{}' does not appear in used_by file '{}'",
                prompt.display(),
                function,
                file
            ),
            Self::OrphanedPrompt { prompt, id } => write!(
                f,
                "{}: id '{}' appears in no source file under src (orphaned prompt)",
                prompt.display(),
                id
            ),
            Self::SourceIo { path, message } => {
                write!(
                    f,
                    "failed to read source at {}: {}",
                    path.display(),
                    message
                )
            }
        }
    }
}

impl std::error::Error for VerifyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Load(err) => Some(err),
            Self::MissingSourceFile { .. }
            | Self::IdNotInFile { .. }
            | Self::FunctionNotInFile { .. }
            | Self::OrphanedPrompt { .. }
            | Self::SourceIo { .. } => None,
        }
    }
}

/// The collected verification failures. Non-empty by construction: it exists
/// only on the error path. `Display` prints one failure per line; `Debug`
/// delegates to it, so a consuming test's `.expect(...)` shows the clean list.
#[derive(Clone, PartialEq, Eq)]
pub struct VerifyReport(Vec<VerifyError>);

impl VerifyReport {
    /// The failures, most-recently-appended last.
    #[must_use]
    pub fn failures(&self) -> &[VerifyError] {
        &self.0
    }
}

impl fmt::Display for VerifyReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, err) in self.0.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{err}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for VerifyReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

impl std::error::Error for VerifyReport {}

#[cfg(test)]
#[path = "tests/verify.rs"]
mod tests;
