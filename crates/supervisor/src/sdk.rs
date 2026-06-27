use anyhow::{Context, Result, bail};
use grizzly_control_api::PROCESS_LABEL_SUFFIX;
use serde_json::json;

/// Thin client for the auto-injected Agones SDK sidecar's REST API (loopback,
/// `http://127.0.0.1:9358` by default). The supervisor takes over the readiness
/// and health duties a busybox sidecar used to perform, plus publishes its
/// process state as a `GameServer` label the bot reads.
#[derive(Clone)]
pub struct SdkClient {
    http: reqwest::Client,
    base_url: String,
}

impl SdkClient {
    #[must_use]
    pub fn new(http: reqwest::Client, base_url: String) -> Self {
        Self { http, base_url }
    }

    /// Signal the `GameServer` is ready to accept players (Agones `Ready()`).
    ///
    /// # Errors
    ///
    /// Returns an error if the SDK request fails or returns a non-success status.
    pub async fn ready(&self) -> Result<()> {
        self.post_empty("/ready").await.context("agones SDK /ready")
    }

    /// Send one health ping (Agones `Health()`); called on a cadence by the
    /// runner's heartbeat task, including while the game is paused.
    ///
    /// # Errors
    ///
    /// Returns an error if the SDK request fails or returns a non-success status.
    pub async fn health(&self) -> Result<()> {
        self.post_empty("/health")
            .await
            .context("agones SDK /health")
    }

    /// Publish the process state as a `GameServer` label (Agones `SetLabel`),
    /// landing as `agones.dev/sdk-grizzly-process=<value>` for the bot to read.
    ///
    /// # Errors
    ///
    /// Returns an error if the SDK request fails or returns a non-success status.
    pub async fn set_process_label(&self, value: &str) -> Result<()> {
        let url = format!("{}/metadata/label", self.base_url);
        let body = json!({ "key": PROCESS_LABEL_SUFFIX, "value": value });
        let response = self
            .http
            .put(&url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("failed to PUT agones SDK label at {url}"))?;
        ensure_success(response, &url).await
    }

    async fn post_empty(&self, path: &str) -> Result<()> {
        let url = format!("{}{path}", self.base_url);
        let response = self
            .http
            .post(&url)
            .json(&json!({}))
            .send()
            .await
            .with_context(|| format!("failed to POST agones SDK at {url}"))?;
        ensure_success(response, &url).await
    }
}

async fn ensure_success(response: reqwest::Response, url: &str) -> Result<()> {
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = response.text().await.unwrap_or_default();
    bail!("agones SDK call to {url} returned {status}: {body}");
}
