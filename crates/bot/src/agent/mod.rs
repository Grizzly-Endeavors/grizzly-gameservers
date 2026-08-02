//! Gary's reusable core: the chat-completions client and the tool-calling loop,
//! both free of Discord and Kubernetes. The Discord-facing shell that supplies
//! tools and wiring lives in `discord::gary`; the in-game shell lives in
//! `ingame::agent`. Each surface renders its own tool results privately, the
//! same as every other tool — see ADR-008. Tool schemas and parameter structs
//! are generated from the `prompts/` tree (prompt-lib); the shared shapes both
//! surfaces reuse live there as `type: params` files (e.g. `NameParams`).

pub(crate) mod llm;
pub(crate) mod recorder;
pub(crate) mod session;
pub(crate) mod store;
pub(crate) mod turn;

pub(crate) use llm::{
    ChatMessage, Completion, OllamaConfig, Role, ToolCall, ToolDef, send_chat_completion,
};
pub(crate) use recorder::TurnRecorder;
pub(crate) use session::{
    DEFAULT_MAX_ROUNDS, Generation, SessionEvent, SessionOutcome, run_session,
};
pub(crate) use store::{SessionId, SessionStore};
pub(crate) use turn::{Environment, TurnActor, TurnIdentity, TurnOrigin};
