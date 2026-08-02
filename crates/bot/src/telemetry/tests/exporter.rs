use super::*;

#[test]
fn the_ingestion_path_is_appended_to_the_host() {
    assert_eq!(
        endpoint_url("http://langfuse-web.langfuse.svc.cluster.local:3000"),
        "http://langfuse-web.langfuse.svc.cluster.local:3000/api/public/otel/v1/traces"
    );
}

/// A host configured with a trailing slash would otherwise produce a double
/// slash, which Langfuse's router treats as a different (missing) route.
#[test]
fn a_trailing_slash_on_the_host_is_dropped() {
    assert_eq!(
        endpoint_url("http://langfuse:3000/"),
        "http://langfuse:3000/api/public/otel/v1/traces"
    );
}

#[test]
fn credentials_are_base64_encoded_as_a_basic_header() {
    // "pk-lf-test:sk-lf-test" in base64.
    assert_eq!(
        basic_auth_header("pk-lf-test", "sk-lf-test"),
        "Basic cGstbGYtdGVzdDpzay1sZi10ZXN0"
    );
}
