use std::sync::Arc;

use axum::Json;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use grizzly_control_api::ControlError;
use tracing::warn;

#[derive(Clone)]
pub struct ControlAuth(Option<Arc<str>>);

impl ControlAuth {
    #[must_use]
    pub fn new(token: Option<&str>) -> Self {
        Self(
            token
                .map(str::trim)
                .filter(|token| !token.is_empty())
                .map(Arc::from),
        )
    }

    #[must_use]
    pub fn is_enforced(&self) -> bool {
        self.0.is_some()
    }

    fn accepts(&self, header_value: Option<&str>) -> bool {
        let Some(expected) = self.0.as_ref() else {
            return true;
        };
        header_value
            .and_then(|value| value.strip_prefix("Bearer "))
            .is_some_and(|presented| constant_time_eq(presented.as_bytes(), expected.as_bytes()))
    }
}

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
