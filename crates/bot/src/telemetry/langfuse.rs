//! Langfuse's OTLP attribute vocabulary.
//!
//! Langfuse reads its own `langfuse.*` namespace in preference to the `OTel` `GenAI`
//! semantic conventions, and it does **not** inherit trace-level attributes
//! across spans — so the trace-level keys below are stamped on every span of a
//! trace, not just the root.

/// Trace-level keys. Repeated on each span of the trace.
pub(crate) const TRACE_NAME: &str = "langfuse.trace.name";
pub(crate) const USER_ID: &str = "langfuse.user.id";
pub(crate) const SESSION_ID: &str = "langfuse.session.id";
pub(crate) const ENVIRONMENT: &str = "langfuse.environment";
pub(crate) const RELEASE: &str = "langfuse.release";
pub(crate) const TAGS: &str = "langfuse.trace.tags";
/// Prefix; the metadata key is appended (`langfuse.trace.metadata.guild`).
pub(crate) const TRACE_METADATA: &str = "langfuse.trace.metadata";

/// Observation-level keys.
pub(crate) const OBSERVATION_TYPE: &str = "langfuse.observation.type";
pub(crate) const INPUT: &str = "langfuse.observation.input";
pub(crate) const OUTPUT: &str = "langfuse.observation.output";
pub(crate) const LEVEL: &str = "langfuse.observation.level";
pub(crate) const STATUS_MESSAGE: &str = "langfuse.observation.status_message";
pub(crate) const MODEL_NAME: &str = "langfuse.observation.model.name";
pub(crate) const USAGE_DETAILS: &str = "langfuse.observation.usage_details";
/// Prefix; the metadata key is appended (`langfuse.observation.metadata.round`).
pub(crate) const OBSERVATION_METADATA: &str = "langfuse.observation.metadata";

/// What kind of node an observation is in the Langfuse tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObservationType {
    /// The turn itself — the trace root.
    Agent,
    /// One model call.
    Generation,
    /// One dispatched tool call.
    Tool,
}

impl ObservationType {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Generation => "generation",
            Self::Tool => "tool",
        }
    }
}

/// Langfuse's severity ladder for an observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Level {
    Default,
    Warning,
    Error,
}

impl Level {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Default => "DEFAULT",
            Self::Warning => "WARNING",
            Self::Error => "ERROR",
        }
    }
}
