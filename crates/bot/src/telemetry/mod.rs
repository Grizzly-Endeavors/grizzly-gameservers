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

pub(crate) struct Telemetry {
    inner: Option<Active>,
    release: String,
}

struct Active {
    processor: BatchSpanProcessor,
    scores: scores::ScoreSender,
}

impl Telemetry {
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

    pub(crate) fn emit(&self, observation: Observation) {
        let Some(active) = self.inner.as_ref() else {
            return;
        };
        active
            .processor
            .on_end(observation.into_span_data(exporter::scope()));
    }

    pub(crate) fn score(&self, post: ScorePost) {
        if let Some(active) = self.inner.as_ref() {
            active.scores.send(post);
        }
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

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

    #[cfg(test)]
    pub(crate) fn flush_for_test(&self) {
        if let Some(active) = self.inner.as_ref() {
            drop(active.processor.force_flush());
        }
    }

    pub(crate) async fn drain(self: Arc<Self>) {
        if self.inner.is_none() {
            return;
        }
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
