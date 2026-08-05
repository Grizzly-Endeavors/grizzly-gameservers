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

#[test]
fn percent_encoding_leaves_the_unreserved_set_alone() {
    assert_eq!(percent_encode("abcXYZ019-_.~"), "abcXYZ019-_.~");
    assert_eq!(percent_encode("a/b?c"), "a%2Fb%3Fc");
    assert_eq!(percent_encode("a b&c=d"), "a%20b%26c%3Dd");
}

/// A flash message is arbitrary text — it carries a setting value or an error —
/// so it has to survive the round trip through a query string intact.
#[test]
fn a_flash_message_survives_the_redirect_it_rides_on() {
    let response = redirect("/settings", &Flash::ok("gary.model is now qwen3 & friends"));
    let location = response
        .headers()
        .get("location")
        .expect("a redirect carries a location")
        .to_str()
        .expect("ascii");
    assert!(location.starts_with("/settings?flash="), "{location}");
    assert!(
        location.contains("%26"),
        "an ampersand must not split the query: {location}"
    );
    assert!(!location.contains("err=1"), "{location}");
}

#[test]
fn an_error_flash_is_marked_as_one() {
    let response = redirect("/settings", &Flash::error("nope"));
    let location = response
        .headers()
        .get("location")
        .expect("a redirect carries a location")
        .to_str()
        .expect("ascii");
    assert!(location.contains("err=1"), "{location}");
}

/// A destination that already carries a query keeps it — the confirmation
/// bounce relies on `?confirm=…` surviving the flash being appended.
#[test]
fn a_flash_appends_to_an_existing_query_string() {
    let response = redirect("/guilds?confirm=open", &Flash::ok("done"));
    let location = response
        .headers()
        .get("location")
        .expect("a redirect carries a location")
        .to_str()
        .expect("ascii");
    assert!(
        location.starts_with("/guilds?confirm=open&flash="),
        "{location}"
    );
}

#[test]
fn a_redirect_without_a_message_carries_no_query() {
    let response = redirect("/guilds?confirm=admit&guild=1", &Flash::none());
    let location = response
        .headers()
        .get("location")
        .expect("a redirect carries a location")
        .to_str()
        .expect("ascii");
    assert_eq!(location, "/guilds?confirm=admit&guild=1");
}

/// The message is operator-supplied text echoed back into the page, so it is
/// escaped like anything else rather than trusted.
#[test]
fn a_flash_message_is_escaped_into_the_page() {
    let flash = Flash::error("<script>alert(1)</script>");
    let rendered = flash.render().into_string();
    assert!(!rendered.contains("<script>"), "{rendered}");
    assert!(rendered.contains("&lt;script&gt;"), "{rendered}");
}
