use super::*;

fn response(status: u16, body: &str) -> reqwest::Response {
    axum::http::Response::builder()
        .status(status)
        .body(body.to_owned())
        .unwrap()
        .into()
}

#[tokio::test]
async fn ensure_success_passes_a_2xx_status() {
    ensure_success(response(200, ""), "http://sdk/ready")
        .await
        .unwrap();
}

#[tokio::test]
async fn ensure_success_bails_with_status_and_body_on_failure() {
    let err = ensure_success(response(500, "sidecar unavailable"), "http://sdk/ready")
        .await
        .unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("500") && message.contains("sidecar unavailable"),
        "error should carry the status and body, got: {message}"
    );
}
