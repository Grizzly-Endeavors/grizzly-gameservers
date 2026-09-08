use super::*;

#[test]
fn short_text_is_returned_whole() {
    assert_eq!(truncate("Jotunn", 80), "Jotunn");
}

#[test]
fn a_long_title_is_cut_and_marked() {
    let title = "a".repeat(200);
    let cut = short_title(&title);
    assert_eq!(cut.chars().count(), 81, "80 kept plus the ellipsis");
    assert!(cut.ends_with('…'));
}

#[test]
fn truncation_counts_characters_not_bytes() {
    // A byte-wise cut would split these and panic; the limit is a character
    // count so multi-byte author text survives it.
    assert_eq!(truncate("日本語テキスト", 3), "日本語…");
    assert_eq!(truncate("émoji 🎮 pack", 7), "émoji 🎮…");
}

#[test]
fn a_description_is_flattened_to_one_line() {
    assert_eq!(
        short_description("adds a\n  better\tmap"),
        "adds a better map"
    );
}

#[test]
fn a_long_description_is_cut_at_two_hundred() {
    let cut = short_description(&"word ".repeat(200));
    assert_eq!(cut.chars().count(), 201);
}

#[test]
fn an_empty_list_names_itself_rather_than_rendering_blank() {
    assert_eq!(list_or_none(&[]), "none listed");
    assert_eq!(
        list_or_none(&["mods".to_owned(), "server-side".to_owned()]),
        "mods, server-side"
    );
}
