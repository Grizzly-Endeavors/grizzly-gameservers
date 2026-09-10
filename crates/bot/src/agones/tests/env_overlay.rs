use super::*;

fn template(env: &str) -> String {
    format!(
        "spec:
  template:
    spec:
      containers:
        - name: game
          env:
{env}
"
    )
}

fn overlay(file: &str, allowlist: &str) -> String {
    template(&format!(
        "            - name: SUPERVISOR_CHILD_ENV_FILE
              value: \"{file}\"
            - name: SUPERVISOR_CHILD_ENV_ALLOWLIST
              value: \"{allowlist}\""
    ))
}

#[test]
fn both_halves_present_declares_an_overlay() {
    assert_eq!(
        parse_env_overlay_from_template(&overlay(".grizzly/env", "SERVER_PASS,SERVER_NAME"))
            .unwrap(),
        Some(EnvOverlay {
            file: ".grizzly/env".to_owned(),
            keys: vec!["SERVER_PASS".to_owned(), "SERVER_NAME".to_owned()],
        })
    );
}

#[test]
fn a_game_declaring_neither_has_no_overlay() {
    assert_eq!(
        parse_env_overlay_from_template(&template("            []")).unwrap(),
        None
    );
}

#[test]
fn a_file_without_an_allowlist_admits_nothing() {
    // Matches the supervisor's own rule: half a catalog edit is not an overlay.
    let yaml = template(
        "            - name: SUPERVISOR_CHILD_ENV_FILE\n              value: \".grizzly/env\"",
    );
    assert_eq!(parse_env_overlay_from_template(&yaml).unwrap(), None);
}

#[test]
fn an_allowlist_without_a_file_has_nothing_to_read() {
    let yaml = template(
        "            - name: SUPERVISOR_CHILD_ENV_ALLOWLIST\n              value: \"SERVER_PASS\"",
    );
    assert_eq!(parse_env_overlay_from_template(&yaml).unwrap(), None);
}

#[test]
fn an_empty_allowlist_is_not_an_overlay() {
    assert_eq!(
        parse_env_overlay_from_template(&overlay(".grizzly/env", "  ,  ,")).unwrap(),
        None
    );
}

#[test]
fn keys_are_trimmed_and_blanks_dropped() {
    let parsed = parse_env_overlay_from_template(&overlay(".grizzly/env", " A , ,B "))
        .unwrap()
        .unwrap();
    assert_eq!(parsed.keys, vec!["A".to_owned(), "B".to_owned()]);
}

#[test]
fn catalog_order_is_preserved() {
    // The prompt reads these back in order, so the catalog's ordering is the
    // one Gary is shown rather than an arbitrary set ordering.
    let parsed = parse_env_overlay_from_template(&overlay(".grizzly/env", "Z,A,M"))
        .unwrap()
        .unwrap();
    assert_eq!(
        parsed.keys,
        vec!["Z".to_owned(), "A".to_owned(), "M".to_owned()]
    );
}

#[test]
fn one_key_renders_without_a_conjunction() {
    let parsed = parse_env_overlay_from_template(&overlay(".grizzly/env", "SERVER_PASS"))
        .unwrap()
        .unwrap();
    assert_eq!(parsed.keys_sentence(), "SERVER_PASS");
}

#[test]
fn two_keys_render_with_and() {
    let parsed = parse_env_overlay_from_template(&overlay(".grizzly/env", "A,B"))
        .unwrap()
        .unwrap();
    assert_eq!(parsed.keys_sentence(), "A and B");
}

#[test]
fn more_keys_render_as_a_list() {
    let parsed = parse_env_overlay_from_template(&overlay(".grizzly/env", "A,B,C,D"))
        .unwrap()
        .unwrap();
    assert_eq!(parsed.keys_sentence(), "A, B, C and D");
}

#[test]
fn malformed_yaml_is_an_error_but_degrades_to_no_overlay() {
    assert!(parse_env_overlay_from_template("spec: [unclosed").is_err());
    assert_eq!(env_overlay_or_warn("valheim", "spec: [unclosed"), None);
}
