use super::*;

#[test]
fn tail_returns_lines_oldest_first() {
    let buffer = LogBuffer::new();
    buffer.push("one".to_owned());
    buffer.push("two".to_owned());
    buffer.push("three".to_owned());
    assert_eq!(
        buffer.tail(2),
        vec!["two", "three"],
        "tail keeps the newest"
    );
    assert_eq!(
        buffer.tail(10),
        vec!["one", "two", "three"],
        "asking for more than present returns all"
    );
}

#[test]
fn tail_of_empty_buffer_is_empty() {
    let buffer = LogBuffer::new();
    assert!(buffer.tail(5).is_empty());
}

#[test]
fn push_past_capacity_drops_oldest() {
    let buffer = LogBuffer::new();
    for i in 0..(CAPACITY + 5) {
        buffer.push(format!("line-{i}"));
    }
    let all = buffer.tail(CAPACITY + 100);
    assert_eq!(all.len(), CAPACITY, "buffer is bounded to its capacity");
    assert_eq!(
        all.first().map(String::as_str),
        Some("line-5"),
        "the five oldest lines should have been evicted"
    );
    assert_eq!(
        all.last().map(String::as_str),
        Some(format!("line-{}", CAPACITY + 4)).as_deref(),
        "the newest line should be retained"
    );
}
