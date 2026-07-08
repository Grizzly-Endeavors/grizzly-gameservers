use super::*;

#[test]
fn all_scope_has_no_label_selector() {
    assert_eq!(ServerScope::All.label_selector(), None);
}

#[test]
fn guild_scope_selects_on_the_guild_label() {
    let selector = ServerScope::Guild("12345".to_owned()).label_selector();
    assert_eq!(
        selector.as_deref(),
        Some("grizzly-gameservers.grizzly-endeavors.com/guild=12345")
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
fn guild_scope_admits_only_its_own_guild() {
    let scope = ServerScope::Guild("42".to_owned());
    assert_eq!(classify(Some("42"), &scope), ScopeVerdict::InScope);
}

#[test]
fn guild_scope_treats_another_guild_as_foreign() {
    let scope = ServerScope::Guild("42".to_owned());
    assert_eq!(classify(Some("7"), &scope), ScopeVerdict::Foreign);
}

#[test]
fn guild_scope_hides_unlabeled_instances() {
    // A pre-scoping server (no guild label) must not leak into a guild view.
    let scope = ServerScope::Guild("42".to_owned());
    assert_eq!(classify(None, &scope), ScopeVerdict::Foreign);
}
