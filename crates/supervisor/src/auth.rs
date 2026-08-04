//! Bearer-token authentication for the control API.
//!
//! Defence in depth behind the network leash. The game-servers `NetworkPolicy`
//! default-denies pod egress and opens exactly one path to the control port — the
//! bot identity → game pods — so reaching this API at all already requires the
//! bot's network identity. That is one control, and it is the kind that fails
//! open: a CNI policy gap, a mislabelled pod, or anything that lands inside the
//! namespace with the right identity inherits full lifecycle and filesystem
//! access to every game pod, with nothing at the application layer to stop it.
//!
//! The token is optional. Absent, the API behaves exactly as the network leash
//! alone allows, so an image or a deployment that hasn't been given one keeps
//! working rather than locking the bot out of every server at once.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use grizzly_control_api::ControlError;
use tracing::warn;

/// The expected credential, or `None` when the control API is unauthenticated.
#[derive(Clone)]
pub struct ControlAuth(Option<Arc<str>>);

impl ControlAuth {
    /// Build from the configured token. An empty string reads as absent, so a
    /// secret that synced blank doesn't become a credential every caller can guess.
    #[must_use]
    pub fn new(token: Option<&str>) -> Self {
        Self(
            token
                .map(str::trim)
                .filter(|token| !token.is_empty())
                .map(Arc::from),
        )
    }

    /// Whether a credential is required.
    #[must_use]
    pub fn is_enforced(&self) -> bool {
        self.0.is_some()
    }

    /// Whether `header_value` carries the expected bearer token.
    fn accepts(&self, header_value: Option<&str>) -> bool {
        let Some(expected) = self.0.as_ref() else {
            return true;
        };
        header_value
            .and_then(|value| value.strip_prefix("Bearer "))
            .is_some_and(|presented| constant_time_eq(presented.as_bytes(), expected.as_bytes()))
    }
}

/// Compare without an early exit, so how long the check takes reveals nothing
/// about how much of the token was right. Hand-rolled rather than pulling in a
/// crate for eight lines — the length check leaks only the length, which is
/// fixed by the generator, not by the attacker.
fn constant_time_eq(presented: &[u8], expected: &[u8]) -> bool {
    if presented.len() != expected.len() {
        return false;
    }
    presented
        .iter()
        .zip(expected)
        .fold(0_u8, |differing, (a, b)| differing | (a ^ b))
        == 0
}

/// Reject any request that doesn't present the configured token.
///
/// Applied to the whole router rather than to the mutating routes alone: `GET
/// /fs/read` returns any file under the instance's data directory and `GET
/// /archive` streams the entire world, so the reads are as worth protecting as
/// the lifecycle calls. Covering everything with one layer also means a route
/// added later is protected by default instead of by remembering to classify it.
pub async fn require_token(
    State(auth): State<ControlAuth>,
    request: Request,
    next: Next,
) -> Response {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    if auth.accepts(presented) {
        return next.run(request).await;
    }
    // Logged at warn: on this network path the only caller that can reach the
    // port is the bot, so a rejection means either a token mismatch between the
    // two halves of a deploy or something else in the namespace probing.
    warn!(
        path = %request.uri().path(),
        "rejected an unauthenticated control request"
    );
    (
        StatusCode::UNAUTHORIZED,
        Json(ControlError::new("unauthorized")),
    )
        .into_response()
}

#[cfg(test)]
#[path = "tests/auth.rs"]
mod tests;
