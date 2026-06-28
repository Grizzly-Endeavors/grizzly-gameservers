//! Gary's reusable core: the chat-completions client and the tool-calling loop,
//! both free of Discord and Kubernetes. The Discord-facing shell that supplies
//! tools and wiring lives in `discord::gary`.

pub(crate) mod llm;
pub(crate) mod session;

pub(crate) use llm::{ChatMessage, OllamaConfig, ToolCall, ToolDef, send_chat_completion};
pub(crate) use session::{DEFAULT_MAX_ROUNDS, SessionOutcome, run_session};
