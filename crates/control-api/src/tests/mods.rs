use super::*;

fn sample() -> ResolvedMods {
    ResolvedMods {
        installed: vec![
            "Advize-PlantEverything-1.20.0".to_owned(),
            "ValheimModding-Jotunn-2.29.2".to_owned(),
        ],
        client_only: vec!["RandyKnapp-EquipmentAndQuickSlots-3.0.2".to_owned()],
        provided: vec!["denikson-BepInExPack_Valheim".to_owned()],
    }
}

#[test]
fn render_puts_installed_releases_on_bare_lines_and_the_rest_in_comments() {
    let text = sample().render();
    assert_eq!(
        text,
        "Advize-PlantEverything-1.20.0\n\
         ValheimModding-Jotunn-2.29.2\n\
         # client-only, not installed here: RandyKnapp-EquipmentAndQuickSlots-3.0.2\n\
         # denikson-BepInExPack_Valheim is supplied by the game image and was not installed\n",
        "the rendered file is the contract the bot parses back"
    );
}

#[test]
fn parse_round_trips_render() {
    let original = sample();
    assert_eq!(
        ResolvedMods::parse(&original.render()),
        original,
        "a file the supervisor wrote must read back as the set it wrote"
    );
}

#[test]
fn parse_ignores_blank_lines_and_unrecognised_comments() {
    let parsed = ResolvedMods::parse(
        "\n\
         # something a later supervisor started writing\n\
         Advize-PlantEverything-1.20.0\n\
         \n",
    );
    assert_eq!(
        parsed.installed,
        vec!["Advize-PlantEverything-1.20.0".to_owned()],
        "an unknown comment costs one entry, never the whole read"
    );
    assert!(
        parsed.client_only.is_empty() && parsed.provided.is_empty(),
        "an unknown comment is not silently filed as a package"
    );
}

#[test]
fn parse_of_an_empty_file_is_an_empty_set() {
    assert_eq!(
        ResolvedMods::parse(""),
        ResolvedMods::default(),
        "a server with no mods resolves to nothing, not to a failure"
    );
}
