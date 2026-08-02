//! Outcome scores, posted to Langfuse's REST API rather than over OTLP.
//!
//! OTLP has no score envelope, so the two planes of a trace arrive by different
//! routes and are joined by the trace id the recorder minted. Posting is
//! deferred to a background task because the recorder's methods are synchronous.

use std::time::Duration;

use opentelemetry::trace::TraceId;
use serde::Serialize;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::config::LangfuseConfig;

/// Scores are tiny and rare (at most two per turn), so a queue this size only
/// fills if Langfuse has been unreachable for a long while.
const QUEUE_DEPTH: usize = 256;

const POST_TIMEOUT: Duration = Duration::from_secs(10);

const SCORES_PATH: &str = "/api/public/scores";

/// A score's value, which decides both the wire encoding and the data type
/// Langfuse files it under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScoreValue {
    /// Langfuse takes booleans as numeric 1/0, not JSON `true`/`false`.
    Boolean(bool),
    Categorical(&'static str),
}

/// One score to attach to an already-exported trace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScorePost {
    pub(crate) trace_id: TraceId,
    pub(crate) name: &'static str,
    pub(crate) value: ScoreValue,
}

/// The wire shape of a score. `id` is Langfuse's idempotency key: deriving it
/// from the trace and score name makes a duplicate post an upsert rather than a
/// second score on the same turn.
#[derive(Debug, PartialEq, Serialize)]
struct ScoreBody {
    id: String,
    #[serde(rename = "traceId")]
    trace_id: String,
    name: &'static str,
    value: serde_json::Value,
    #[serde(rename = "dataType")]
    data_type: &'static str,
}

impl From<&ScorePost> for ScoreBody {
    fn from(post: &ScorePost) -> Self {
        let (value, data_type) = match post.value {
            ScoreValue::Boolean(flag) => (serde_json::Value::from(u8::from(flag)), "BOOLEAN"),
            ScoreValue::Categorical(label) => (serde_json::Value::from(label), "CATEGORICAL"),
        };
        Self {
            id: format!("{}-{}", post.trace_id, post.name),
            trace_id: post.trace_id.to_string(),
            name: post.name,
            value,
            data_type,
        }
    }
}

/// Queues scores for the background poster. Cloneable so the recorder can hold
/// one per turn without an `Arc`.
#[derive(Clone, Debug)]
pub(crate) struct ScoreSender(mpsc::Sender<ScorePost>);

impl ScoreSender {
    /// Queue a score, dropping it if the poster is backed up. A turn never waits
    /// on telemetry, so a full queue costs one score, not a stalled reply.
    pub(crate) fn send(&self, post: ScorePost) {
        if let Err(err) = self.0.try_send(post) {
            debug!(error = %err, "langfuse score queue is full; dropping a score");
        }
    }

    /// A sender with no poster behind it, for tests that exercise span export.
    #[cfg(test)]
    pub(crate) fn discarding() -> Self {
        let (tx, _) = mpsc::channel(1);
        Self(tx)
    }
}

/// Build the sender and the poster that drains it. The caller spawns the poster
/// into the shutdown-tracked task set, so queued scores are posted before the
/// process exits.
pub(crate) fn channel(
    http: reqwest::Client,
    cfg: &LangfuseConfig,
    shutdown: CancellationToken,
) -> (ScoreSender, Poster) {
    let (tx, rx) = mpsc::channel(QUEUE_DEPTH);
    let poster = Poster {
        http,
        url: format!("{}{SCORES_PATH}", cfg.host.trim_end_matches('/')),
        public_key: cfg.public_key.clone(),
        secret_key: cfg.secret_key.clone(),
        rx,
        shutdown,
    };
    (ScoreSender(tx), poster)
}

/// The background loop that posts queued scores.
pub(crate) struct Poster {
    http: reqwest::Client,
    url: String,
    public_key: String,
    secret_key: String,
    rx: mpsc::Receiver<ScorePost>,
    shutdown: CancellationToken,
}

impl Poster {
    /// Drain the queue until shutdown, then drain what's left so a redeploy —
    /// which happens on every merge — doesn't lose the scores of turns that were
    /// in flight.
    pub(crate) async fn run(mut self) {
        let mut failing = false;
        loop {
            let post = tokio::select! {
                post = self.rx.recv() => post,
                () = self.shutdown.cancelled() => break,
            };
            let Some(post) = post else { break };
            failing = self.post(&post, failing).await;
        }
        self.rx.close();
        while let Some(post) = self.rx.recv().await {
            failing = self.post(&post, failing).await;
        }
    }

    /// Post one score, logging only when the healthy/failing state flips so an
    /// unreachable Langfuse doesn't produce a line per score.
    async fn post(&self, post: &ScorePost, failing: bool) -> bool {
        let body = ScoreBody::from(post);
        let sent = self
            .http
            .post(&self.url)
            .basic_auth(&self.public_key, Some(&self.secret_key))
            .timeout(POST_TIMEOUT)
            .json(&body)
            .send()
            .await;
        match sent {
            Ok(response) if response.status().is_success() => {
                if failing {
                    info!("langfuse scores are posting again");
                }
                false
            }
            Ok(response) => {
                Self::note_failure(failing, &response.status().to_string(), post.name);
                true
            }
            Err(err) => {
                Self::note_failure(failing, &format!("{err}"), post.name);
                true
            }
        }
    }

    fn note_failure(already_failing: bool, cause: &str, name: &str) {
        if !already_failing {
            warn!(
                cause,
                score = name,
                "failed to post a score to langfuse; suppressing further reports until it recovers"
            );
        }
    }
}

#[cfg(test)]
#[path = "tests/scores.rs"]
mod tests;
