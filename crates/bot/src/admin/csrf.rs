use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tracing::warn;

#[derive(Clone, Debug)]
pub(crate) struct ExpectedOrigin {
    pub(crate) origin: String,
}

pub(crate) fn is_same_origin(headers: &HeaderMap, expected: &str) -> bool {
    if let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        return origin.trim_end_matches('/') == expected.trim_end_matches('/');
    }
    if let Some(referer) = headers
        .get(header::REFERER)
        .and_then(|value| value.to_str().ok())
    {
        return origin_of(referer).is_some_and(|origin| origin == expected.trim_end_matches('/'));
    }
    false
}

fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    (!host.is_empty()).then(|| format!("{scheme}://{host}"))
}

pub(crate) async fn require_same_origin(
    State(expected): State<ExpectedOrigin>,
    request: Request,
    next: Next,
) -> Response {
    let mutating = !matches!(
        *request.method(),
        axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS
    );
    if mutating && !is_same_origin(request.headers(), &expected.origin) {
        warn!(
            path = %request.uri().path(),
            "refused a cross-origin write to the admin panel"
        );
        return (
            StatusCode::FORBIDDEN,
            "This request didn't come from the admin panel. Open it directly and try again.",
        )
            .into_response();
    }
    next.run(request).await
}

#[cfg(test)]
#[path = "tests/csrf.rs"]
mod tests;
