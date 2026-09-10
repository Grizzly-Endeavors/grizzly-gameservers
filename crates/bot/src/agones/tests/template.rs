use super::*;

fn template(env: &str) -> String {
    format!(
        "apiVersion: agones.dev/v1
kind: GameServer
metadata:
  annotations:
    example.com/thing: value
spec:
  template:
    spec:
      containers:
        - name: game
          env:
{env}
"
    )
}

#[test]
fn annotations_are_read_off_the_metadata() {
    let parsed = AnnotatedTemplate::parse(&template("            []")).unwrap();
    assert_eq!(
        parsed
            .annotations()
            .and_then(|map| map.get("example.com/thing")),
        Some(&"value".to_owned())
    );
}

#[test]
fn a_template_with_no_annotations_reports_none() {
    let parsed = AnnotatedTemplate::parse("spec: {}").unwrap();
    assert_eq!(parsed.annotations(), None);
}

#[test]
fn env_value_reads_a_literal() {
    let parsed = AnnotatedTemplate::parse(&template(
        "            - name: A\n              value: hello",
    ))
    .unwrap();
    assert_eq!(parsed.env_value("A"), Some("hello"));
}

#[test]
fn env_value_trims_surrounding_whitespace() {
    let parsed = AnnotatedTemplate::parse(&template(
        "            - name: A\n              value: \"  hello  \"",
    ))
    .unwrap();
    assert_eq!(parsed.env_value("A"), Some("hello"));
}

#[test]
fn an_empty_value_reads_as_absent() {
    // A template that declares a variable as "" means "declared but off"; every
    // per-game fact read through here treats that the same as undeclared.
    let parsed = AnnotatedTemplate::parse(&template(
        "            - name: A\n              value: \"   \"",
    ))
    .unwrap();
    assert_eq!(parsed.env_value("A"), None);
}

#[test]
fn a_value_from_variable_is_never_returned() {
    // It names a secret, and no per-game fact read through here is one.
    let parsed = AnnotatedTemplate::parse(&template(
        "            - name: TOKEN
              valueFrom:
                secretKeyRef:
                  name: s
                  key: k",
    ))
    .unwrap();
    assert_eq!(parsed.env_value("TOKEN"), None);
}

#[test]
fn an_undeclared_variable_reports_none() {
    let parsed = AnnotatedTemplate::parse(&template(
        "            - name: A\n              value: hello",
    ))
    .unwrap();
    assert_eq!(parsed.env_value("B"), None);
}

#[test]
fn every_container_is_searched() {
    let parsed = AnnotatedTemplate::parse(
        "spec:
  template:
    spec:
      containers:
        - name: first
          env:
            - name: A
              value: one
        - name: second
          env:
            - name: B
              value: two
",
    )
    .unwrap();
    assert_eq!(parsed.env_value("B"), Some("two"));
}

#[test]
fn unrelated_manifest_fields_do_not_perturb_the_parse() {
    // The struct is deliberately partial; a catalog edit elsewhere in the
    // manifest must not turn into a parse error.
    let parsed = AnnotatedTemplate::parse(
        "spec:
  health:
    failureThreshold: 10
  ports:
    - name: game
      containerPort: 2456
  template:
    spec:
      terminationGracePeriodSeconds: 150
      containers:
        - name: game
          resources:
            limits:
              cpu: \"4\"
          env:
            - name: A
              value: one
",
    )
    .unwrap();
    assert_eq!(parsed.env_value("A"), Some("one"));
}

#[test]
fn malformed_yaml_is_an_error() {
    assert!(AnnotatedTemplate::parse("spec: [unclosed").is_err());
}
