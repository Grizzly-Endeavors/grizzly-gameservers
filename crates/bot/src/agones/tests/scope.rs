use super::*;

#[test]
fn all_scope_has_no_label_selector() {
    assert_eq!(ServerScope::All.label_selector(), None);
}

#[test]
fn channel_scope_selects_on_the_channel_label() {
    let selector = ServerScope::Channel("12345".to_owned()).label_selector();
    assert_eq!(
        selector.as_deref(),
        Some("grizzly-gameservers.grizzly-endeavors.com/channel=12345")
    );
}

#[test]
fn all_scope_admits_every_instance_including_unlabeled() {
    assert_eq!(
        classify(Some("999"), &ServerScope::All),
        ScopeVerdict::InScope
    );
    assert_eq!(classify(None, &ServerScope::All), ScopeVerdict::InScope);
}

#[test]
fn channel_scope_admits_only_its_own_channel() {
    let scope = ServerScope::Channel("42".to_owned());
    assert_eq!(classify(Some("42"), &scope), ScopeVerdict::InScope);
}

#[test]
fn channel_scope_treats_another_channel_as_foreign() {
    let scope = ServerScope::Channel("42".to_owned());
    assert_eq!(classify(Some("7"), &scope), ScopeVerdict::Foreign);
}

#[test]
fn channel_scope_hides_unlabeled_instances() {
    // A pre-scoping server (no channel label) must not leak into a channel view.
    let scope = ServerScope::Channel("42".to_owned());
    assert_eq!(classify(None, &scope), ScopeVerdict::Foreign);
}
