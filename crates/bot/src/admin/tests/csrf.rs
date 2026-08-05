use super::*;

use axum::http::HeaderValue;

const PANEL: &str = "https://gameservers-admin.example";

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in pairs {
        headers.insert(
            axum::http::HeaderName::from_bytes(name.as_bytes()).expect("valid header name"),
            HeaderValue::from_str(value).expect("valid header value"),
        );
    }
    headers
}

#[test]
fn a_write_from_the_panel_itself_is_allowed() {
    assert!(is_same_origin(&headers(&[("origin", PANEL)]), PANEL));
}

#[test]
fn a_trailing_slash_on_either_side_does_not_change_the_origin() {
    assert!(is_same_origin(
        &headers(&[("origin", "https://gameservers-admin.example/")]),
        PANEL
    ));
    assert!(is_same_origin(
        &headers(&[("origin", PANEL)]),
        "https://gameservers-admin.example/"
    ));
}

#[test]
fn a_write_from_another_site_is_refused() {
    for origin in [
        "https://evil.example",
        "http://gameservers-admin.example",
        "https://gameservers-admin.example.evil.test",
        "null",
    ] {
        assert!(
            !is_same_origin(&headers(&[("origin", origin)]), PANEL),
            "{origin} must not be treated as the panel"
        );
    }
}

/// `Referer` is the fallback for the navigations that historically omitted
/// `Origin`; only its scheme-and-host half counts.
#[test]
fn referer_stands_in_for_a_missing_origin() {
    assert!(is_same_origin(
        &headers(&[(
            "referer",
            "https://gameservers-admin.example/settings?flash=hi"
        )]),
        PANEL
    ));
    assert!(!is_same_origin(
        &headers(&[("referer", "https://evil.example/settings")]),
        PANEL
    ));
}

/// `Origin` wins when both are present: a page can influence `Referer` far more
/// easily than it can forge `Origin`.
#[test]
fn origin_is_preferred_over_referer() {
    assert!(!is_same_origin(
        &headers(&[
            ("origin", "https://evil.example"),
            ("referer", "https://gameservers-admin.example/settings"),
        ]),
        PANEL
    ));
}

/// A state-changing request with no provenance at all is exactly the shape this
/// check exists to stop, so absence is a refusal rather than a pass.
#[test]
fn a_write_with_no_provenance_is_refused() {
    assert!(!is_same_origin(&headers(&[]), PANEL));
}
