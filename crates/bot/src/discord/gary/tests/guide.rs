use super::*;

const CATALOG: &str = "factorio, minecraft, valheim";

fn overlay() -> EnvOverlay {
    EnvOverlay {
        file: ".grizzly/env".to_owned(),
        keys: vec![
            "SERVER_PASS".to_owned(),
            "SERVER_NAME".to_owned(),
            "SERVER_PUBLIC".to_owned(),
            "CROSSPLAY".to_owned(),
        ],
    }
}

fn thunderstore() -> ModRegistry {
    ModRegistry::Thunderstore {
        community: "valheim".to_owned(),
        provided: vec!["denikson-BepInExPack_Valheim".to_owned()],
    }
}

#[test]
fn a_modrinth_guide_names_the_game_and_the_file_installs_go_through() {
    let registry = ModRegistry::Modrinth;
    let rendered = render_guide(
        "minecraft",
        &GuideSubject::Registry(&registry),
        None,
        CATALOG,
    );
    assert!(rendered.contains("minecraft"), "{rendered}");
    assert!(rendered.contains(".grizzly/env"), "{rendered}");
    assert!(rendered.contains("MODRINTH_PROJECTS"), "{rendered}");
}

#[test]
fn a_thunderstore_guide_teaches_pinning_and_delete_the_line() {
    let registry = thunderstore();
    let rendered = render_guide("valheim", &GuideSubject::Registry(&registry), None, CATALOG);
    assert!(rendered.contains(".grizzly/mods"), "{rendered}");
    assert!(rendered.contains("Namespace-Name-1.2.3"), "{rendered}");
    assert!(rendered.contains("delete its line"), "{rendered}");
    assert!(rendered.contains("opposite"), "{rendered}");
}

#[test]
fn the_two_registries_never_hand_back_each_others_instructions() {
    let modrinth = ModRegistry::Modrinth;
    let thunder = thunderstore();
    let one = render_guide(
        "minecraft",
        &GuideSubject::Registry(&modrinth),
        None,
        CATALOG,
    );
    let other = render_guide("valheim", &GuideSubject::Registry(&thunder), None, CATALOG);
    assert!(!one.contains(".grizzly/mods"), "{one}");
    assert!(!other.contains("MODRINTH_PROJECTS"), "{other}");
}

#[test]
fn a_guide_reads_as_intro_then_choice_then_install() {
    let registry = thunderstore();
    let rendered = render_guide("valheim", &GuideSubject::Registry(&registry), None, CATALOG);
    let intro = rendered
        .find("Here is how mods work")
        .expect("the intro leads");
    let choice = rendered
        .find("Never name a mod from memory")
        .expect("choosing comes next");
    let install = rendered
        .find("Installing is a config change")
        .expect("then installing");
    assert!(intro < choice && choice < install, "{rendered}");
}

#[test]
fn a_game_with_no_registry_says_so_and_forbids_improvising() {
    let rendered = render_guide("factorio", &GuideSubject::NoRegistry, None, CATALOG);
    assert!(rendered.contains("factorio"), "{rendered}");
    assert!(rendered.contains("no supported way"), "{rendered}");
    assert!(rendered.contains("Don't improvise"), "{rendered}");
    assert!(!rendered.contains("search_mods"), "{rendered}");
}

#[test]
fn an_unknown_game_names_the_catalog_and_how_to_find_the_right_id() {
    let rendered = render_guide("valhiem", &GuideSubject::UnknownGame, None, CATALOG);
    assert!(rendered.contains("valhiem"), "{rendered}");
    assert!(rendered.contains(CATALOG), "{rendered}");
    assert!(rendered.contains("about_server"), "{rendered}");
}

#[test]
fn an_empty_catalog_renders_as_words_rather_than_a_blank() {
    let rendered = render_guide("anything", &GuideSubject::UnknownGame, None, "(none)");
    assert!(rendered.contains("(none)"), "{rendered}");
}

#[test]
fn a_thunderstore_guide_carries_the_profile_code_workflow() {
    let guide = render_guide(
        "valheim",
        &GuideSubject::Registry(&thunderstore()),
        None,
        CATALOG,
    );
    assert!(guide.contains("import_profile"), "{guide}");
    assert!(guide.contains("export_profile"), "{guide}");
    assert!(guide.contains("client:"), "{guide}");
}

#[test]
fn a_modrinth_guide_says_nothing_about_profile_codes() {
    let guide = render_guide(
        "minecraft",
        &GuideSubject::Registry(&ModRegistry::Modrinth),
        None,
        CATALOG,
    );
    assert!(!guide.contains("profile code"), "{guide}");
}

#[test]
fn a_game_with_an_overlay_names_the_file_and_only_its_own_keys() {
    let overlay = overlay();
    let rendered = render_guide(
        "valheim",
        &GuideSubject::Registry(&thunderstore()),
        Some(&overlay),
        CATALOG,
    );
    assert!(rendered.contains(".grizzly/env"), "{rendered}");
    assert!(
        rendered.contains("SERVER_PASS, SERVER_NAME, SERVER_PUBLIC and CROSSPLAY"),
        "{rendered}"
    );
}

#[test]
fn an_overlay_guide_says_the_setting_is_in_no_file() {
    let overlay = overlay();
    let rendered = render_guide(
        "valheim",
        &GuideSubject::Registry(&thunderstore()),
        Some(&overlay),
        CATALOG,
    );
    assert!(
        rendered.contains("not stored in any config file"),
        "{rendered}"
    );
    assert!(rendered.contains("there isn't one"), "{rendered}");
}

#[test]
fn a_game_without_an_overlay_is_pointed_at_its_own_config_files() {
    let rendered = render_guide("factorio", &GuideSubject::NoRegistry, None, CATALOG);
    assert!(rendered.contains("factorio"), "{rendered}");
    assert!(rendered.contains("browse_files"), "{rendered}");
    assert!(!rendered.contains(".grizzly/env"), "{rendered}");
}

#[test]
fn every_real_game_answers_the_settings_question() {
    let declared = overlay();
    let registry = thunderstore();
    for (subject, overlay) in [
        (GuideSubject::NoRegistry, None),
        (GuideSubject::NoRegistry, Some(&declared)),
        (GuideSubject::Registry(&registry), None),
        (GuideSubject::Registry(&registry), Some(&declared)),
    ] {
        let rendered = render_guide("valheim", &subject, overlay, CATALOG);
        assert!(
            rendered.contains("settings"),
            "settings section missing: {rendered}"
        );
    }
}

#[test]
fn settings_lead_and_the_mod_workflow_follows() {
    let overlay = overlay();
    let rendered = render_guide(
        "valheim",
        &GuideSubject::Registry(&thunderstore()),
        Some(&overlay),
        CATALOG,
    );
    let settings = rendered
        .find("not stored in any config file")
        .expect("settings lead");
    let mods = rendered.find("Here is how mods work").expect("mods follow");
    assert!(settings < mods, "{rendered}");
}

#[test]
fn an_unknown_game_is_still_only_the_correction() {
    let overlay = overlay();
    let rendered = render_guide(
        "valhiem",
        &GuideSubject::UnknownGame,
        Some(&overlay),
        CATALOG,
    );
    assert!(rendered.contains("valhiem"), "{rendered}");
    assert!(!rendered.contains(".grizzly/env"), "{rendered}");
    assert!(!rendered.contains("browse_files"), "{rendered}");
}
