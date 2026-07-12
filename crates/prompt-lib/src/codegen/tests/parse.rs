use super::*;

const SIMPLE: &str = "---
id: Greeting
type: prompt
---
Hello world.";

fn parse(content: &str) -> Result<RawFile, PromptError> {
    parse_file(Path::new("Greeting.md"), content)
}

#[test]
fn parses_frontmatter_and_body() {
    let raw = parse(SIMPLE).unwrap();
    assert_eq!(raw.id.as_deref(), Some("Greeting"));
    assert_eq!(raw.kind.as_deref(), Some("prompt"));
    assert_eq!(raw.body, "Hello world.");
}

#[test]
fn missing_delimiters_is_frontmatter_error() {
    let err = parse("no frontmatter at all").unwrap_err();
    assert!(
        matches!(err, PromptError::Frontmatter { .. }),
        "got {err:?}"
    );
}

#[test]
fn trims_exactly_one_trailing_newline() {
    let raw = parse("---\nid: Greeting\ntype: prompt\n---\nHello world.\n").unwrap();
    assert_eq!(raw.body, "Hello world.");
}

#[test]
fn rejects_leading_body_whitespace() {
    let err = parse("---\nid: Greeting\ntype: prompt\n---\n  indented").unwrap_err();
    assert!(
        matches!(err, PromptError::BodyWhitespace { .. }),
        "got {err:?}"
    );
}

#[test]
fn rejects_trailing_body_whitespace() {
    // A double trailing newline survives the single-newline trim as trailing ws.
    let err = parse("---\nid: Greeting\ntype: prompt\n---\nHello\n\n").unwrap_err();
    assert!(
        matches!(err, PromptError::BodyWhitespace { .. }),
        "got {err:?}"
    );
}

#[test]
fn handles_body_without_closing_newline() {
    let raw = parse("---\nid: Greeting\ntype: prompt\n---\nno trailing newline").unwrap();
    assert_eq!(raw.body, "no trailing newline");
}

#[test]
fn empty_body_is_allowed_by_parse() {
    let raw = parse("---\nid: NameParams\ntype: params\n---\n").unwrap();
    assert_eq!(raw.body, "");
}

#[test]
fn tokenizes_literals_and_placeholders() {
    let segments = tokenize_body("Hi {{name}}!").unwrap();
    assert_eq!(
        segments,
        vec![
            BodySegment::Literal("Hi ".to_owned()),
            BodySegment::Placeholder("name".to_owned()),
            BodySegment::Literal("!".to_owned()),
        ]
    );
}

#[test]
fn tokenizes_adjacent_placeholders() {
    let segments = tokenize_body("{{a}}{{b}}").unwrap();
    assert_eq!(
        segments,
        vec![
            BodySegment::Placeholder("a".to_owned()),
            BodySegment::Placeholder("b".to_owned()),
        ]
    );
}

#[test]
fn tokenizes_plain_text_as_one_literal() {
    let segments = tokenize_body("just text").unwrap();
    assert_eq!(segments, vec![BodySegment::Literal("just text".to_owned())]);
}

#[test]
fn unterminated_placeholder_is_an_error() {
    let err = tokenize_body("open {{name").unwrap_err();
    assert_eq!(err, TokenizeError::Unterminated);
}
