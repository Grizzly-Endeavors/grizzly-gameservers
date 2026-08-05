//! Keeping another site from acting through a signed-in admin's browser.
//!
//! The panel has no server-side session to hang a synchroniser token on, so it
//! leans on two independent properties instead:
//!
//! 1. **The outpost's session cookie is `SameSite=Lax`.** A form POST from
//!    another origin arrives without it, so the ingress' auth subrequest fails
//!    and the request never reaches this process at all. This is the load-bearing
//!    half, and it holds without any code here.
//! 2. **`Origin` must match the panel's own host.** Browsers send `Origin` on
//!    every state-changing request and a page cannot forge it. This is the belt
//!    to the cookie's braces: it keeps working if the cookie policy ever loosens,
//!    and it costs one header comparison.
//!
//! A token scheme would additionally survive a browser that sent neither header,
//! which no browser in use does. It is not worth the session store for a panel
//! with one user.

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tracing::warn;

/// The origin this panel is served from, e.g. `https://gameservers-admin.example`.
#[derive(Clone, Debug)]
pub(crate) struct ExpectedOrigin {
    pub(crate) origin: String,
}

/// Whether a state-changing request came from the panel itself.
///
/// `Origin` is preferred; `Referer` is the fallback for the handful of
/// navigations that historically omitted `Origin`. Neither present is a refusal
/// rather than a pass — a state-changing request with no provenance at all is
/// exactly the shape this exists to stop.
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

/// The scheme-and-host prefix of a URL, without its path.
fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    (!host.is_empty()).then(|| format!("{scheme}://{host}"))
}

/// Refuse a mutating request that didn't come from the panel.
///
/// Read-only requests pass untouched: they change nothing, and gating them would
/// break an ordinary bookmark, which sends no `Origin`.
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
