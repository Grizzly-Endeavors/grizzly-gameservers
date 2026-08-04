//! The HTTP client the bot dials game-pod supervisors with.
//!
//! A distinct type from `reqwest::Client` on purpose. The bot's general client
//! also talks to Ollama and Langfuse, so a bearer token attached to it as a
//! default header would be sent off-cluster to third parties. Making the
//! supervisor client its own type means the credential is attached in exactly one
//! place, and the compiler — not a reviewer — is what stops supervisor traffic
//! from going out over the general client or vice versa.

use reqwest::{Client, RequestBuilder};

/// A `reqwest::Client` that presents the shared control credential on every
/// request. Cheap to clone: `reqwest::Client` is an `Arc` internally.
#[derive(Clone)]
pub(crate) struct ControlClient {
    inner: Client,
    /// `Authorization: Bearer <token>`, pre-rendered, or `None` when the control
    /// API is unauthenticated. Rendering once keeps the per-request cost to a
    /// header clone.
    credential: Option<String>,
}

impl ControlClient {
    /// Wrap `inner`, presenting `token` on every request when one is configured.
    /// A blank token reads as absent, matching the supervisor's own parsing, so a
    /// secret that synced empty doesn't become a credential.
    pub(crate) fn new(inner: Client, token: Option<&str>) -> Self {
        Self {
            inner,
            credential: token
                .map(str::trim)
                .filter(|token| !token.is_empty())
                .map(|token| format!("Bearer {token}")),
        }
    }

    /// Whether a credential is being presented.
    pub(crate) fn is_authenticated(&self) -> bool {
        self.credential.is_some()
    }

    pub(crate) fn get(&self, url: &str) -> RequestBuilder {
        self.authorize(self.inner.get(url))
    }

    pub(crate) fn post(&self, url: &str) -> RequestBuilder {
        self.authorize(self.inner.post(url))
    }

    fn authorize(&self, request: RequestBuilder) -> RequestBuilder {
        match self.credential.as_deref() {
            Some(credential) => request.header(reqwest::header::AUTHORIZATION, credential),
            None => request,
        }
    }
}

#[cfg(test)]
#[path = "tests/control_client.rs"]
mod tests;
