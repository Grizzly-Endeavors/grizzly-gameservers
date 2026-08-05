use super::*;

/// The person reading this has signed in successfully and been turned away
/// anyway, so the page has to name the group they're missing — otherwise the
/// only way to find out is to ask.
#[test]
fn the_denial_page_names_the_group_and_the_user() {
    let rendered = denial_body(&Denial::NotAMember {
        username: "friend".to_owned(),
        required: "grizzly-admins".to_owned(),
        held: vec!["grizzly-users".to_owned()],
    })
    .into_string();
    assert!(rendered.contains("grizzly-admins"), "{rendered}");
    assert!(rendered.contains("friend"), "{rendered}");
    assert!(rendered.contains("grizzly-users"), "{rendered}");
}

#[test]
fn the_unidentified_page_does_not_blame_the_reader() {
    let rendered = denial_body(&Denial::Unidentified).into_string();
    assert!(rendered.contains("sign-in"), "{rendered}");
    // No group name to show — there is no identity to compare against one.
    assert!(!rendered.contains("grizzly-admins"), "{rendered}");
}

#[test]
fn a_user_in_no_groups_is_told_so_plainly() {
    let rendered = denial_body(&Denial::NotAMember {
        username: "newcomer".to_owned(),
        required: "grizzly-admins".to_owned(),
        held: Vec::new(),
    })
    .into_string();
    assert!(rendered.contains("not in any groups"), "{rendered}");
}

/// Names and group lists arrive from an identity provider, so they are not
/// trusted markup — escaping them is what keeps a display name from becoming a
/// script tag on the one page only admins see.
#[test]
fn identity_text_is_escaped_into_the_page() {
    let rendered = denial_body(&Denial::NotAMember {
        username: "<script>alert(1)</script>".to_owned(),
        required: "grizzly-admins".to_owned(),
        held: vec!["<img onerror=x>".to_owned()],
    })
    .into_string();
    assert!(!rendered.contains("<script>"), "{rendered}");
    assert!(!rendered.contains("<img onerror"), "{rendered}");
    assert!(rendered.contains("&lt;script&gt;"), "{rendered}");
}

#[test]
fn a_table_with_no_rows_says_so_instead_of_showing_headers() {
    let rendered = table(&["One", "Two"], Vec::new(), "Nothing here yet.").into_string();
    assert!(rendered.contains("Nothing here yet."), "{rendered}");
    assert!(!rendered.contains("<thead>"), "{rendered}");
}

#[test]
fn a_table_with_rows_renders_its_headers() {
    let rows = vec![html! { tr { td { "a" } } }];
    let rendered = table(&["One"], rows, "empty").into_string();
    assert!(rendered.contains("<thead>"), "{rendered}");
    assert!(rendered.contains("One"), "{rendered}");
    assert!(!rendered.contains("empty"), "{rendered}");
}

#[test]
fn availability_reads_as_a_configuration_fact_not_an_error() {
    assert!(availability(true).into_string().contains("on"));
    let off = availability(false).into_string();
    assert!(off.contains("off"), "{off}");
    // Neutral, not red: an optional subsystem that isn't wired is a choice.
    assert!(!off.contains("badge bad"), "{off}");
}

#[test]
fn the_active_nav_entry_is_the_only_one_marked() {
    let identity = AdminIdentity {
        username: "bear".to_owned(),
        email: None,
    };
    let rendered = page("Settings", Nav::Settings, &identity, &html! {}).into_string();
    assert_eq!(
        rendered.matches("class=\"active\"").count(),
        1,
        "{rendered}"
    );
    assert!(rendered.contains("bear"), "{rendered}");
}
