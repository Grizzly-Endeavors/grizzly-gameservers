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

#[test]
fn an_install_line_is_pinned_or_floating() {
    let ModsLine::Install(Ok(pinned)) = classify("ValheimModding-Jotunn-2.30.0") else {
        panic!("a pinned release is an install line");
    };
    assert_eq!(pinned.id.to_string(), "ValheimModding-Jotunn");
    assert_eq!(pinned.pin.unwrap().to_string(), "2.30.0");

    let ModsLine::Install(Ok(floating)) = classify("ValheimModding-Jotunn") else {
        panic!("a bare package is an install line");
    };
    assert!(
        floating.pin.is_none(),
        "an unpinned line takes the latest at resolve time"
    );
}

#[test]
fn blank_and_comment_lines_are_ignored() {
    for line in ["", "   ", "# a note", "  # indented"] {
        assert_eq!(classify(line), ModsLine::Ignored, "{line:?}");
    }
}

#[test]
fn a_client_line_must_carry_a_version() {
    let ModsLine::Client(Ok(release)) = classify("client:Advize-PlantEasily-2.2.0") else {
        panic!("a pinned client entry is usable");
    };
    assert_eq!(release.to_string(), "Advize-PlantEasily-2.2.0");

    // Unpinned is its own problem: the fix is to add a version, and telling
    // someone the line is malformed would send them hunting a typo.
    assert_eq!(
        classify("client:Advize-PlantEasily"),
        ModsLine::Client(Err(ClientLineProblem::Unpinned(
            "Advize-PlantEasily".parse().unwrap()
        )))
    );
    assert!(matches!(
        classify("client:nonsense"),
        ModsLine::Client(Err(ClientLineProblem::Unusable(_)))
    ));
}

#[test]
fn a_malformed_install_line_reports_the_pinned_shape() {
    let ModsLine::Install(Err(reason)) = classify("not a mod") else {
        panic!("garbage on the install side is a malformed install line");
    };
    assert!(
        reason.to_string().contains("Namespace-Name-1.2.3"),
        "got: {reason}"
    );
}

#[test]
fn whitespace_around_a_line_is_insignificant() {
    assert_eq!(
        classify("  ValheimModding-Jotunn-2.30.0  "),
        classify("ValheimModding-Jotunn-2.30.0")
    );
    assert_eq!(
        classify("client: Advize-PlantEasily-2.2.0"),
        classify("client:Advize-PlantEasily-2.2.0")
    );
}

#[test]
fn rendering_a_line_round_trips_through_classify() {
    let id = "ValheimModding-Jotunn".parse().unwrap();
    let version = "2.30.0".parse().unwrap();
    let release = "Advize-PlantEasily-2.2.0".parse().unwrap();

    assert_eq!(
        classify(&render_install(&id, Some(version))),
        ModsLine::Install(Ok(InstallLine {
            id: id.clone(),
            pin: Some(version)
        }))
    );
    assert_eq!(
        classify(&render_install(&id, None)),
        ModsLine::Install(Ok(InstallLine { id, pin: None }))
    );
    assert_eq!(
        classify(&render_client(&release)),
        ModsLine::Client(Ok(release))
    );
}
