//! Exports Gary's turns to Langfuse: spans over OTLP, outcome scores over REST.
//!
//! The handle is always constructed; absent credentials produce a disabled one
//! that accepts and discards everything, so callers never branch on whether
//! telemetry is configured — the same graceful-degradation shape as the database
//! and the deferred-task queue.

mod exporter;
pub(crate) mod ids;
pub(crate) mod langfuse;
mod scores;
mod span;

use std::future::Future;
use std::sync::Arc;

use opentelemetry_sdk::trace::{BatchSpanProcessor, SpanProcessor as _};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

pub(crate) use langfuse::{Level, ObservationType};
pub(crate) use scores::{ScorePost, ScoreValue};
pub(crate) use span::Observation;

use crate::config::LangfuseConfig;

/// The turn-telemetry sink. Disabled when Langfuse isn't configured or its
/// exporter couldn't be built.
pub(crate) struct Telemetry {
    inner: Option<Active>,
    /// Which build this process is. Known regardless of whether export is on, and
    /// stamped on every span so a behaviour change can be pinned to a deploy.
    release: String,
}

struct Active {
    processor: BatchSpanProcessor,
    scores: scores::ScoreSender,
}

impl Telemetry {
    /// Build the exporter and the score poster, or a disabled sink.
    ///
    /// Never fails: a missing key pair disables turn telemetry with one log line
    /// and leaves every other behaviour intact. The returned future is the score
    /// poster's loop, which the caller spawns into the shutdown-tracked task set
    /// (it is inert when telemetry is disabled).
    pub(crate) fn new(
        config: Option<&LangfuseConfig>,
        release: String,
        http: reqwest::Client,
        shutdown: CancellationToken,
    ) -> (Self, impl Future<Output = ()> + Send + 'static) {
        let (inner, poster) = Self::build(config, http, shutdown);
        (Self { inner, release }, async move {
            if let Some(poster) = poster {
                poster.run().await;
            }
        })
    }

    /// Which build this process is, for the `release` attribute.
    pub(crate) fn release(&self) -> &str {
        &self.release
    }

    fn build(
        config: Option<&LangfuseConfig>,
        http: reqwest::Client,
        shutdown: CancellationToken,
    ) -> (Option<Active>, Option<scores::Poster>) {
        let Some(config) = config else {
            warn!(
                "LANGFUSE_PUBLIC_KEY/SECRET_KEY not set; turn telemetry disabled \
                 (everything else still works)"
            );
            return (None, None);
        };
        match exporter::build(config) {
            Ok(processor) => {
                let (scores, poster) = scores::channel(http, config, shutdown);
                info!(host = %config.host, "turn telemetry enabled (langfuse)");
                (Some(Active { processor, scores }), Some(poster))
            }
            Err(err) => {
                error!(
                    error = ?err,
                    host = %config.host,
                    "could not build the langfuse exporter; turn telemetry disabled"
                );
                (None, None)
            }
        }
    }

    /// Hand a finished observation to the batching exporter. Non-blocking: the
    /// processor enqueues and its own thread does the shipping, so a turn never
    /// waits on telemetry.
    pub(crate) fn emit(&self, observation: Observation) {
        let Some(active) = self.inner.as_ref() else {
            return;
        };
        active
            .processor
            .on_end(observation.into_span_data(exporter::scope()));
    }

    /// Queue an outcome score against an already-exported trace.
    pub(crate) fn score(&self, post: ScorePost) {
        if let Some(active) = self.inner.as_ref() {
            active.scores.send(post);
        }
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

    /// An enabled sink over a caller-supplied processor, so a test can assert on
    /// the exported span tree without a collector. Scores are discarded.
    #[cfg(test)]
    pub(crate) fn for_test(processor: BatchSpanProcessor, release: &str) -> Self {
        Self {
            inner: Some(Active {
                processor,
                scores: scores::ScoreSender::discarding(),
            }),
            release: release.to_owned(),
        }
    }

    /// Push buffered spans to the exporter without stopping it.
    #[cfg(test)]
    pub(crate) fn flush_for_test(&self) {
        if let Some(active) = self.inner.as_ref() {
            drop(active.processor.force_flush());
        }
    }

    /// Flush buffered spans and stop the exporter. Called once, after the
    /// shutdown drain has awaited in-flight turns, so the spans those turns
    /// produced are already queued.
    /// Takes `Arc<Self>` rather than `self` because the handle is shared with
    /// both Gary surfaces, which are still alive at shutdown.
    pub(crate) async fn drain(self: Arc<Self>) {
        if self.inner.is_none() {
            return;
        }
        // Both calls block the caller on the exporter's background thread, which
        // must not happen on a runtime worker.
        let joined = tokio::task::spawn_blocking(move || {
            let Some(active) = self.inner.as_ref() else {
                return (Ok(()), Ok(()));
            };
            let flushed = active.processor.force_flush();
            let stopped = active.processor.shutdown();
            (flushed, stopped)
        })
        .await;
        match joined {
            Ok((Ok(()), Ok(()))) => info!("turn telemetry flushed and shut down"),
            Ok((flushed, stopped)) => warn!(
                ?flushed,
                ?stopped,
                "turn telemetry drain was incomplete; buffered spans may be lost"
            ),
            Err(err) => warn!(error = ?err, "turn telemetry drain task failed"),
        }
    }
}
