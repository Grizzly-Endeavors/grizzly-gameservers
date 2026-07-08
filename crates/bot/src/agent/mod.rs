//! Gary's reusable core: the chat-completions client and the tool-calling loop,
//! both free of Discord and Kubernetes. The Discord-facing shell that supplies
//! tools and wiring lives in `discord::gary`. The [`render`] submodule holds the
//! server-summary and error copy shared by both Gary surfaces (in-game chat and
//! Discord) so the two don't drift.

pub(crate) mod llm;
pub(crate) mod render;
pub(crate) mod session;
pub(crate) mod store;

pub(crate) use llm::{ChatMessage, OllamaConfig, ToolCall, ToolDef, send_chat_completion};
pub(crate) use render::{GarySurface, cluster_error, format_server_list, format_summary, no_such};
pub(crate) use session::{DEFAULT_MAX_ROUNDS, SessionEvent, SessionOutcome, run_session};
pub(crate) use store::SessionStore;
