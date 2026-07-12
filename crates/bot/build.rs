//! Compile the crate's `prompts/` tree into `$OUT_DIR/prompts.rs` (included by
//! `src/prompts.rs`). `emit` validates every prompt file and registers the tree
//! for rebuild-on-change, so a prompt edit is picked up by the next build.

use std::path::Path;

fn main() -> Result<(), grizzly_prompt_lib::PromptError> {
    let prompts = Path::new(env!("CARGO_MANIFEST_DIR")).join("prompts");
    grizzly_prompt_lib::emit(&prompts)
}
