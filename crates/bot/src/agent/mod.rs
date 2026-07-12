//! Gary's reusable core: the chat-completions client and the tool-calling loop,
//! both free of Discord and Kubernetes. The Discord-facing shell that supplies
//! tools and wiring lives in `discord::gary`; the in-game shell lives in
//! `ingame::agent`. Each surface renders its own tool results privately, the
//! same as every other tool — see ADR-008. The [`tools`] submodule holds only
//! what's genuinely surface-agnostic: the tool-parameter type and schema
//! builders.

pub(crate) mod llm;
pub(crate) mod session;
pub(crate) mod store;
pub(crate) mod tools;

pub(crate) use llm::{ChatMessage, OllamaConfig, Role, ToolCall, ToolDef, send_chat_completion};
pub(crate) use session::{DEFAULT_MAX_ROUNDS, SessionEvent, SessionOutcome, run_session};
pub(crate) use store::SessionStore;
pub(crate) use tools::{NameParams, no_args_schema, params_schema};
