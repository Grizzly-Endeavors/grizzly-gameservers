//! The payload stored in the deferred-task queue: one task a user asked Gary to
//! run once a server reaches a condition. Serialized to JSON as a Redis list
//! element (the server and condition live in the list *key*, not here). Carries
//! the originating identity so a fired batch can be delivered back to the right
//! channel and attributed for logging.

use serde::{Deserialize, Serialize};

/// One queued task for a `(server, condition)` pair.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct DeferredTask {
    /// The natural-language instruction to carry out when the condition is met.
    pub(crate) task: String,
    /// The Discord user who queued it (attribution/logging; the batch itself runs
    /// at the manager tier, not this user's tier).
    pub(crate) requested_by: u64,
    /// The channel the request came from — where the outcome is posted back.
    pub(crate) channel_id: u64,
    /// The guild the request came from, if any.
    pub(crate) guild_id: Option<u64>,
    /// Unix seconds when it was queued, for ordering/debugging.
    pub(crate) requested_at: i64,
}

impl DeferredTask {
    pub(crate) fn new(
        task: impl Into<String>,
        requested_by: u64,
        channel_id: u64,
        guild_id: Option<u64>,
    ) -> Self {
        Self {
            task: task.into(),
            requested_by,
            channel_id,
            guild_id,
            requested_at: jiff::Timestamp::now().as_second(),
        }
    }
}
