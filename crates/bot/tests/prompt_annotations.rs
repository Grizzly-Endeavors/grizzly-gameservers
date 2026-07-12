#![expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live at crate root by cargo convention"
)]
use std::path::Path;

use grizzly_prompt_lib::verify_annotations;

#[test]
fn prompt_annotations_are_current() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    verify_annotations(&root.join("prompts"), &root.join("src"))
        .expect("prompt annotations are stale — fix the named prompt file's used_by entry");
}
