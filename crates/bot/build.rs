use std::path::Path;

fn main() -> Result<(), grizzly_agent_prompts::PromptError> {
    let prompts = Path::new(env!("CARGO_MANIFEST_DIR")).join("prompts");
    grizzly_agent_prompts::emit(&prompts)
}
