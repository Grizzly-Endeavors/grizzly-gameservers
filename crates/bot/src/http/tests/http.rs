use super::*;
use axum::http::HeaderValue;

fn headers_with_auth(value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, HeaderValue::from_str(value).unwrap());
    headers
}

#[test]
fn open_endpoint_accepts_any_caller() {
    assert!(
        authorized(None, &HeaderMap::new()),
        "with no token configured the endpoint runs open"
    );
}

#[test]
fn rejects_missing_token_when_one_is_expected() {
    assert!(!authorized(Some("s3cret"), &HeaderMap::new()));
}

#[test]
fn rejects_wrong_token() {
    assert!(!authorized(
        Some("s3cret"),
        &headers_with_auth("Bearer nope")
    ));
}

#[test]
fn rejects_token_without_bearer_scheme() {
    assert!(!authorized(Some("s3cret"), &headers_with_auth("s3cret")));
}

#[test]
fn accepts_correct_bearer_token() {
    assert!(authorized(
        Some("s3cret"),
        &headers_with_auth("Bearer s3cret")
    ));
}

#[tokio::test]
async fn health_reports_ok_regardless_of_state() {
    assert_eq!(
        health().await,
        StatusCode::OK,
        "health must answer 200 independent of Gary or gateway state"
    );
}

#[test]
fn constant_time_eq_matches_only_identical_bytes() {
    assert!(constant_time_eq(b"abc", b"abc"));
    assert!(!constant_time_eq(b"abc", b"abd"));
    assert!(
        !constant_time_eq(b"abc", b"abcd"),
        "different lengths never match"
    );
    assert!(constant_time_eq(b"", b""));
}
