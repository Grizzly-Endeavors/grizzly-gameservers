use super::*;

#[test]
fn accepts_valid_ids() {
    for id in [
        "EditFile",
        "HttpProxy",
        "A",
        "S3Bucket",
        "RunWhen",
        "NameParams",
    ] {
        assert!(is_valid_id(id), "expected '{id}' to be a valid id");
    }
}

#[test]
fn rejects_invalid_ids() {
    for id in [
        "editFile",
        "Edit_File",
        "Edit-File",
        "",
        "3Edit",
        "Edit File",
    ] {
        assert!(!is_valid_id(id), "expected '{id}' to be rejected");
    }
}

#[test]
fn pascal_to_snake_lowercases_each_segment() {
    assert_eq!(pascal_to_snake("HttpProxy"), "http_proxy");
    assert_eq!(pascal_to_snake("EditFile"), "edit_file");
    assert_eq!(pascal_to_snake("RunWhen"), "run_when");
    assert_eq!(pascal_to_snake("S3Bucket"), "s3_bucket");
    assert_eq!(pascal_to_snake("A"), "a");
}

#[test]
fn snake_to_pascal_capitalizes_each_segment() {
    assert_eq!(snake_to_pascal("run_when"), "RunWhen");
    assert_eq!(snake_to_pascal("wait_idle"), "WaitIdle");
    assert_eq!(snake_to_pascal("name"), "Name");
}

#[test]
fn case_conversion_round_trips() {
    for snake in ["run_when", "name", "http_proxy", "wait_idle"] {
        assert_eq!(pascal_to_snake(&snake_to_pascal(snake)), snake);
    }
}

#[test]
fn accepts_valid_placeholder_names() {
    for name in ["name", "game_id", "x1", "server"] {
        assert!(is_valid_placeholder_name(name), "expected '{name}' valid");
    }
}

#[test]
fn rejects_invalid_placeholder_names() {
    // Uppercase, digit-leading, dash, keywords, and empty are all illegal.
    for name in ["Name", "1name", "game-id", "type", "self", "match", ""] {
        assert!(
            !is_valid_placeholder_name(name),
            "expected '{name}' rejected"
        );
    }
}

#[test]
fn enum_values_allow_keywords_but_enforce_grammar() {
    for value in ["wait_idle", "empty", "x1", "type"] {
        assert!(is_valid_enum_value(value), "expected '{value}' valid");
    }
    for value in ["WaitIdle", "1x", "wait-idle", ""] {
        assert!(!is_valid_enum_value(value), "expected '{value}' rejected");
    }
}
