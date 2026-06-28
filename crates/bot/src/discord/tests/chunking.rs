use super::*;

#[test]
fn empty_input() {
    let result = chunk_text("", 100);
    assert!(result.is_empty(), "empty input should return empty vec");
}

#[test]
fn short_text_no_split() {
    let result = chunk_text("hello world", 100);
    assert_eq!(result.len(), 1, "short text should not be split");
    assert_eq!(
        result.first().map(String::as_str),
        Some("hello world"),
        "content should be preserved"
    );
}

#[test]
fn exact_boundary_no_split() {
    let text = "abcde";
    let result = chunk_text(text, 5);
    assert_eq!(result.len(), 1, "text exactly at limit should not be split");
}

#[test]
fn splits_at_newline() {
    let text = "line one\nline two\nline three";
    let result = chunk_text(text, 15);
    assert!(result.len() >= 2, "should split into multiple chunks");
    for chunk in &result {
        assert!(
            chunk.len() <= 15,
            "chunk should fit in limit, got len {}",
            chunk.len()
        );
    }
    let combined = result.join("\n");
    assert_eq!(
        combined, text,
        "chunks joined with newline should equal original"
    );
}

#[test]
fn splits_at_whitespace() {
    let text = "word1 word2 word3 word4 word5";
    let result = chunk_text(text, 12);
    assert!(result.len() >= 2, "should split into multiple chunks");
    let combined: String = result.join("");
    assert_eq!(combined, text, "chunks joined should equal original");
}

#[test]
fn long_single_line_hard_split() {
    let text = "a".repeat(50);
    let result = chunk_text(&text, 20);
    assert!(
        result.len() >= 3,
        "should split long line into multiple chunks"
    );
    let combined: String = result.join("");
    assert_eq!(combined, text, "recombined chunks should equal original");
}

#[test]
fn code_fence_closed_and_reopened() {
    let text = "before\n```rust\nline 1\nline 2\nline 3\nline 4\n```\nafter";
    let result = chunk_text(text, 30);
    assert!(result.len() >= 2, "should split into multiple chunks");

    for (i, chunk) in result.iter().enumerate() {
        let opens: usize = chunk
            .lines()
            .filter(|l| {
                let t = l.trim();
                t.starts_with("```") && t.len() > 3
            })
            .count();
        let closes: usize = chunk.lines().filter(|l| l.trim() == "```").count();
        assert!(
            opens.abs_diff(closes) <= 1,
            "chunk {i} has unbalanced fences: opens={opens} closes={closes}\n---\n{chunk}\n---"
        );
    }

    let all_chunks = result.concat();
    for line in text.lines() {
        assert!(
            all_chunks.contains(line),
            "original line '{line}' should survive in chunked output"
        );
    }
}

#[test]
fn unicode_boundary_respect() {
    let text = "hello 🌍 world 🌍 test";
    let result = chunk_text(text, 10);
    assert!(result.len() >= 2, "should split unicode text");
}

#[test]
fn zero_max_chars() {
    let result = chunk_text("hello", 0);
    assert!(result.is_empty(), "zero max should return empty vec");
}

#[test]
fn single_char_input() {
    let result = chunk_text("a", 100);
    assert_eq!(result.len(), 1, "single char should not be split");
    assert_eq!(
        result.first().map(String::as_str),
        Some("a"),
        "content should be preserved"
    );
}

#[test]
fn no_fence_just_text_splits_cleanly() {
    let text = "aaa\nbbb\nccc\nddd\neee";
    let result = chunk_text(text, 8);
    assert!(result.len() >= 2, "should split into multiple chunks");
    for chunk in &result {
        assert!(
            chunk.len() <= 8,
            "chunk should fit in limit, got len {} for: {chunk:?}",
            chunk.len()
        );
    }
}
