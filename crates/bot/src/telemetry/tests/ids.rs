use super::*;

#[test]
fn minted_trace_ids_are_valid_and_distinct() {
    let first = mint_trace_id();
    let second = mint_trace_id();
    assert_ne!(first, TraceId::INVALID);
    assert_ne!(second, TraceId::INVALID);
    assert_ne!(first, second);
}

#[test]
fn minted_span_ids_are_valid_and_distinct() {
    let first = mint_span_id();
    let second = mint_span_id();
    assert_ne!(first, SpanId::INVALID);
    assert_ne!(second, SpanId::INVALID);
    assert_ne!(first, second);
}

/// The rendered trace id is what the scores REST API is keyed on, so its shape
/// is a contract, not a formatting detail.
#[test]
fn trace_id_renders_as_32_lowercase_hex() {
    let rendered = mint_trace_id().to_string();
    assert_eq!(rendered.len(), 32);
    assert!(
        rendered
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "unexpected characters in {rendered}"
    );
}

#[test]
fn span_id_renders_as_16_lowercase_hex() {
    let rendered = mint_span_id().to_string();
    assert_eq!(rendered.len(), 16);
    assert!(
        rendered
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "unexpected characters in {rendered}"
    );
}

#[test]
fn all_zero_bytes_are_replaced() {
    assert_eq!(non_zero([0_u8; 4]), [1_u8; 4]);
}

#[test]
fn non_zero_leaves_real_randomness_alone() {
    assert_eq!(non_zero([0, 0, 7, 0]), [0, 0, 7, 0]);
}
