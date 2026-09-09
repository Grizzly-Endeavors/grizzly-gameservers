//! Gary's reusable core: the chat-completions client and the tool-calling loop,
//! both free of Discord and Kubernetes. The Discord-facing shell that supplies
//! tools and wiring lives in `discord::gary`; the in-game shell lives in
//! `ingame::agent`. Each surface renders its own tool results privately, the
//! same as every other tool — see ADR-008. Tool schemas and parameter structs
//! are generated from the `prompts/` tree (prompt-lib); the shared shapes both
//! surfaces reuse live there as `type: params` files (e.g. `NameParams`).

pub(crate) mod llm;
pub(crate) mod observations;
pub(crate) mod recorder;
pub(crate) mod session;
pub(crate) mod store;
pub(crate) mod turn;

pub(crate) use llm::{
    ChatMessage, Completion, OllamaConfig, Role, ToolCall, ToolDef, send_chat_completion,
};
pub(crate) use recorder::TurnRecorder;
pub(crate) use session::{
    DEFAULT_MAX_ROUNDS, EscalationReason, EscalationRequest, Generation, SessionCallbacks,
    SessionEvent, SessionOutcome, run_session,
};
pub(crate) use store::{
    DEFAULT_TRANSCRIPT_TARGET_TOKENS, DEFAULT_TRANSCRIPT_TRIGGER_TOKENS, SessionId, SessionKey,
    SessionStore, TranscriptLimits,
};
pub(crate) use turn::{Environment, TurnActor, TurnIdentity, TurnOrigin};

/// Persist a finished turn's transcript under the conversation it belongs to,
/// sized against the settings in force for that turn.
///
/// Wraps [`SessionStore::commit`] so the three surfaces that finish a turn don't
/// each re-derive the measured prompt size from the outcome — a detail that
/// belongs with the loop, not with a Discord handler.
pub(crate) fn commit_turn(
    sessions: &SessionStore,
    key: SessionKey,
    id: SessionId,
    messages: Vec<ChatMessage>,
    outcome: &SessionOutcome,
    limits: TranscriptLimits,
) {
    let measured = outcome.measured_prompt_tokens();
    sessions.commit(
        key,
        id,
        messages,
        std::time::Instant::now(),
        measured,
        limits,
    );
}
