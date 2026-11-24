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

const INGESTION_VERSION_HEADER: &str = "x-langfuse-ingestion-version";
const INGESTION_VERSION: &str = "4";

/// Well under the SDK's 512 default: every generation observation carries the
/// whole transcript it sent, so a full batch at the default size is a
/// multi-megabyte POST.
const MAX_EXPORT_BATCH: usize = 16;

const MAX_QUEUE: usize = 2048;

const SCHEDULED_DELAY: Duration = Duration::from_secs(5);
const EXPORT_TIMEOUT: Duration = Duration::from_secs(10);

const SERVICE_NAME: &str = "grizzly-gameservers";

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
    processor.set_resource(&Resource::builder().with_service_name(SERVICE_NAME).build());
    Ok(processor)
}

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
