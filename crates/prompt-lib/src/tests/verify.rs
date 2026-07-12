use super::*;

/// A valid no-variable prompt whose id is `Greeting`, used by `greet`.
const GREETING: &str = "---
id: Greeting
type: prompt
annotations:
  sent_when: at conversation start
  used_by:
    - file: discord.rs
      function: greet
  reasoning:
    - opens every conversation
---
Hello there.";

/// A second valid prompt (`Farewell`, used by `bye`) for the aggregation case.
const FAREWELL: &str = "---
id: Farewell
type: prompt
annotations:
  sent_when: at conversation end
  used_by:
    - file: present.rs
      function: bye
  reasoning:
    - closes the conversation
---
Goodbye.";

/// Write each `(relative_path, content)` into a fresh tempdir.
fn write_tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (rel, content) in files {
        let path = dir.path().join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, content).unwrap();
    }
    dir
}

/// Build a prompt tree and a source tree, then verify one against the other.
/// Both tempdirs stay alive for the duration of the call.
fn run(prompts: &[(&str, &str)], src: &[(&str, &str)]) -> Result<(), VerifyReport> {
    let prompts_dir = write_tree(prompts);
    let src_dir = write_tree(src);
    verify_annotations(prompts_dir.path(), src_dir.path())
}

#[test]
fn passes_when_used_by_file_names_id_and_function() {
    let result = run(
        &[("Greeting.md", GREETING)],
        &[("discord.rs", "fn greet() -> String { Greeting::render() }")],
    );
    assert!(
        result.is_ok(),
        "unexpected failures: {}",
        result.unwrap_err()
    );
}

#[test]
fn resolves_used_by_paths_in_subdirectories() {
    let greeting = GREETING.replace("file: discord.rs", "file: gary/discord.rs");
    let result = run(
        &[("Greeting.md", greeting.as_str())],
        &[("gary/discord.rs", "fn greet() { Greeting::render(); }")],
    );
    assert!(
        result.is_ok(),
        "unexpected failures: {}",
        result.unwrap_err()
    );
}

#[test]
fn flags_missing_source_file() {
    // `discord.rs` is never created; a different file carries the id so the
    // prompt is not also flagged as orphaned.
    let report = run(
        &[("Greeting.md", GREETING)],
        &[("elsewhere.rs", "// Greeting is referenced here")],
    )
    .unwrap_err();
    let failures = report.failures();
    assert_eq!(failures.len(), 1, "expected one failure, got: {report}");
    assert!(
        matches!(
            failures.first(),
            Some(VerifyError::MissingSourceFile { .. })
        ),
        "got: {report}"
    );
}

#[test]
fn flags_used_by_file_missing_the_id() {
    // `discord.rs` has the function but not the id; another file carries the id
    // so orphan does not fire — isolating the id-in-file check.
    let report = run(
        &[("Greeting.md", GREETING)],
        &[
            ("discord.rs", "fn greet() {}"),
            ("elsewhere.rs", "// Greeting lives here"),
        ],
    )
    .unwrap_err();
    let failures = report.failures();
    assert_eq!(failures.len(), 1, "expected one failure, got: {report}");
    assert!(
        matches!(failures.first(), Some(VerifyError::IdNotInFile { .. })),
        "got: {report}"
    );
}

#[test]
fn flags_used_by_file_missing_the_function() {
    // `discord.rs` mentions the id but not `greet`.
    let report = run(
        &[("Greeting.md", GREETING)],
        &[("discord.rs", "fn other() { Greeting::render(); }")],
    )
    .unwrap_err();
    let failures = report.failures();
    assert_eq!(failures.len(), 1, "expected one failure, got: {report}");
    assert!(
        matches!(
            failures.first(),
            Some(VerifyError::FunctionNotInFile { .. })
        ),
        "got: {report}"
    );
}

#[test]
fn flags_orphaned_prompt() {
    // The id appears in no source file. Orphan fires alongside IdNotInFile (the
    // used_by file necessarily lacks the id too), so assert it is present rather
    // than the sole failure.
    let report = run(
        &[("Greeting.md", GREETING)],
        &[("discord.rs", "fn greet() {}")],
    )
    .unwrap_err();
    assert!(
        report
            .failures()
            .iter()
            .any(|e| matches!(e, VerifyError::OrphanedPrompt { .. })),
        "expected an orphaned-prompt failure, got: {report}"
    );
}

#[test]
fn aggregates_failures_across_prompts() {
    let report = run(
        &[("Greeting.md", GREETING), ("Farewell.md", FAREWELL)],
        &[
            // Carries both ids so neither prompt is orphaned.
            ("refs.rs", "// Greeting Farewell"),
            // Has `bye` but not the `Farewell` id → IdNotInFile for Farewell.
            ("present.rs", "fn bye() {}"),
            // Greeting's used_by file `discord.rs` is absent → MissingSourceFile.
        ],
    )
    .unwrap_err();
    let failures = report.failures();
    assert_eq!(failures.len(), 2, "expected two failures, got: {report}");
    assert!(
        failures
            .iter()
            .any(|e| matches!(e, VerifyError::MissingSourceFile { .. })),
        "missing the MissingSourceFile failure: {report}"
    );
    assert!(
        failures
            .iter()
            .any(|e| matches!(e, VerifyError::IdNotInFile { .. })),
        "missing the IdNotInFile failure: {report}"
    );
}

#[test]
fn propagates_load_failure() {
    // A prompt missing its `reasoning` annotation fails to load; verify surfaces
    // that as a single Load failure without touching the source tree.
    let broken = "---
id: Broken
type: prompt
annotations:
  sent_when: whenever
  used_by:
    - file: x.rs
      function: f
---
Body.";
    let report = run(&[("Broken.md", broken)], &[]).unwrap_err();
    let failures = report.failures();
    assert_eq!(failures.len(), 1, "expected one failure, got: {report}");
    assert!(
        matches!(failures.first(), Some(VerifyError::Load(_))),
        "got: {report}"
    );
}
