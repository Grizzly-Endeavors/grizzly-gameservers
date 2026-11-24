pub(crate) mod llm;
pub(crate) mod observations;
pub(crate) mod recorder;
pub(crate) mod session;
pub(crate) mod store;
pub(crate) mod turn;

pub(crate) use llm::{
    ChatMessage, Completion, OllamaConfig, Role, TokenUsage, ToolCall, ToolDef,
    send_chat_completion,
};
pub(crate) use recorder::TurnRecorder;
pub(crate) use session::{
    DEFAULT_MAX_ROUNDS, EscalationReason, EscalationRequest, Generation, SessionCallbacks,
    SessionEvent, SessionOutcome, TurnEnding, run_session,
};
pub(crate) use store::{
    DEFAULT_TRANSCRIPT_TARGET_TOKENS, DEFAULT_TRANSCRIPT_TRIGGER_TOKENS, SessionId, SessionKey,
    SessionStore, TranscriptLimits,
};
pub(crate) use turn::{Environment, TurnActor, TurnIdentity, TurnOrigin};

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
