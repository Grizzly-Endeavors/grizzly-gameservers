//! Builds the OTLP/HTTP span pipeline pointed at Langfuse's ingestion endpoint.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context as _, Result};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use opentelemetry::InstrumentationScope;
use opentelemetry_otlp::{Protocol, SpanExporter, WithExportConfig as _, WithHttpConfig as _};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::{BatchConfigBuilder, BatchSpanProcessor, SpanProcessor as _};

use crate::config::LangfuseConfig;

/// Langfuse's OTLP trace path. Shipped in full because the exporter appends
/// `/v1/traces` only to an endpoint it read from `OTEL_EXPORTER_OTLP_ENDPOINT`;
/// a programmatically-set endpoint is used verbatim, so a bare host POSTs to the
/// wrong path and 404s.
const OTLP_TRACES_PATH: &str = "/api/public/otel/v1/traces";

/// Asks Langfuse v4 to make a trace visible as it arrives rather than on the
/// legacy batch path.
const INGESTION_VERSION_HEADER: &str = "x-langfuse-ingestion-version";
const INGESTION_VERSION: &str = "4";

/// Well under the SDK's 512 default: every generation observation carries the
/// whole transcript it sent, so a full batch at the default size is a
/// multi-megabyte POST.
const MAX_EXPORT_BATCH: usize = 16;

/// Ceiling on spans buffered while the exporter is behind or unreachable. Past
/// it, spans are dropped rather than allowed to grow the bot's memory — losing
/// visibility is the design's accepted failure mode, stalling a turn is not.
const MAX_QUEUE: usize = 2048;

const SCHEDULED_DELAY: Duration = Duration::from_secs(5);
const EXPORT_TIMEOUT: Duration = Duration::from_secs(10);

/// Identifies this bot as the producer on every exported span.
const SERVICE_NAME: &str = "grizzly-gameservers";

/// Assemble the batching span processor. The returned processor owns a dedicated
/// background thread, so nothing here needs a Tokio runtime.
///
/// # Errors
///
/// Returns an error if the exporter's HTTP client can't be constructed.
pub(crate) fn build(cfg: &LangfuseConfig) -> Result<BatchSpanProcessor> {
    let endpoint = endpoint_url(&cfg.host);
    let mut headers = HashMap::new();
    headers.insert(
        "Authorization".to_owned(),
        basic_auth_header(&cfg.public_key, &cfg.secret_key),
    );
    headers.insert(
        INGESTION_VERSION_HEADER.to_owned(),
        INGESTION_VERSION.to_owned(),
    );

    let exporter = SpanExporter::builder()
        .with_http()
        .with_endpoint(&endpoint)
        // Protocol and timeout are pinned rather than left to inference so a
        // stray OTEL_EXPORTER_OTLP_* in the pod env can't re-route this to gRPC,
        // which Langfuse doesn't accept.
        .with_protocol(Protocol::HttpBinary)
        .with_timeout(EXPORT_TIMEOUT)
        .with_headers(headers)
        .build()
        .with_context(|| format!("failed to build the langfuse otlp exporter for {endpoint}"))?;

    let batch = BatchConfigBuilder::default()
        .with_max_queue_size(MAX_QUEUE)
        .with_max_export_batch_size(MAX_EXPORT_BATCH)
        .with_scheduled_delay(SCHEDULED_DELAY)
        .build();
    let mut processor = BatchSpanProcessor::builder(exporter)
        .with_batch_config(batch)
        .build();
    // Normally a tracer provider does this. There isn't one in this pipeline, so
    // without it every span would export with an empty resource.
    processor.set_resource(&Resource::builder().with_service_name(SERVICE_NAME).build());
    Ok(processor)
}

/// The instrumentation scope stamped on every observation this bot produces.
pub(crate) fn scope() -> InstrumentationScope {
    InstrumentationScope::builder(concat!(env!("CARGO_PKG_NAME"), ".turn"))
        .with_version(env!("CARGO_PKG_VERSION"))
        .build()
}

fn endpoint_url(host: &str) -> String {
    format!("{}{OTLP_TRACES_PATH}", host.trim_end_matches('/'))
}

fn basic_auth_header(public_key: &str, secret_key: &str) -> String {
    format!(
        "Basic {}",
        BASE64.encode(format!("{public_key}:{secret_key}"))
    )
}

#[cfg(test)]
#[path = "tests/exporter.rs"]
mod tests;
