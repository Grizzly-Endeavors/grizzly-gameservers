use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct DeferredTask {
    pub(crate) task: String,
    pub(crate) requested_by: u64,
    pub(crate) channel_id: u64,
    pub(crate) guild_id: Option<u64>,
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
