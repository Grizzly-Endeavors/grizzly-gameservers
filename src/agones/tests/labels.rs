use std::collections::BTreeMap;

use super::*;

fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

#[test]
fn managed_label_is_recognized() {
    let map = labels(&[(MANAGED_BY_KEY, MANAGED_BY_VALUE)]);
    assert!(is_managed(Some(&map)));
}

#[test]
fn other_managed_by_value_is_not_ours() {
    let map = labels(&[(MANAGED_BY_KEY, "flux")]);
    assert!(
        !is_managed(Some(&map)),
        "the GitOps singleton must be refused"
    );
}

#[test]
fn missing_labels_are_not_managed() {
    assert!(!is_managed(None));
    assert!(!is_managed(Some(&BTreeMap::new())));
}
