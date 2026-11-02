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
    // The two rules a model can't infer: a search result carries no version, and
    // removal here is the exact opposite of Minecraft's rule.
    let registry = thunderstore();
    let rendered = render_guide("valheim", &GuideSubject::Registry(&registry), None, CATALOG);
    assert!(rendered.contains(".grizzly/mods"), "{rendered}");
    assert!(rendered.contains("Namespace-Name-1.2.3"), "{rendered}");
    assert!(rendered.contains("delete its line"), "{rendered}");
    assert!(rendered.contains("opposite"), "{rendered}");
}

#[test]
fn the_two_registries_never_hand_back_each_others_instructions() {
    // Carrying one game's habits to the other is the failure this tool exists to
    // prevent, so neither guide may mention the other's file.
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
    // The alternative to a final answer is Gary hand-installing through the file
    // tools, which puts the change outside everything that would check it.
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
    // The fast path in both directions, and the one thing a code cannot do —
    // follow the pack after it changes.
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
    // They are an r2modman idea; offering one on Minecraft would send a friend
    // looking for a launcher feature that doesn't exist there.
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
    // The whole point: these settings are in no config file, so the guide has
    // to say where they are instead of leaving the search to run.
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
    // Without this the standing "browse for the file that holds a setting"
    // instruction wins and the search runs anyway.
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
    // Silence here would read as "this game has no settings" beside the other
    // branch's explicit file.
    let rendered = render_guide("factorio", &GuideSubject::NoRegistry, None, CATALOG);
    assert!(rendered.contains("factorio"), "{rendered}");
    assert!(rendered.contains("browse_files"), "{rendered}");
    assert!(!rendered.contains(".grizzly/env"), "{rendered}");
}

#[test]
fn every_real_game_answers_the_settings_question() {
    // A model that calls game_guide before changing a setting must never be
    // told nothing and left to guess which branch it is in.
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
    // A question about a password should reach its answer before the guide
    // starts talking about mods.
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
    // A typo has no settings and no mods to describe; adding either would bury
    // the one useful answer.
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

fn catalog_overlay(game: &str) -> EnvOverlay {
    let yaml = std::fs::read_to_string(format!(
        "{}/../../games/{game}/gameserver.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    crate::agones::parse_env_overlay_from_template(&yaml)
        .unwrap()
        .unwrap()
}

#[test]
fn the_valheim_guide_names_the_world_switch_key() {
    let overlay = catalog_overlay("valheim");
    let rendered = render_guide(
        "valheim",
        &GuideSubject::Registry(&thunderstore()),
        Some(&overlay),
        CATALOG,
    );
    assert!(rendered.contains("WORLD_NAME"), "{rendered}");
}

#[test]
fn the_factorio_guide_takes_the_overlay_branch_and_names_the_save_keys() {
    let overlay = catalog_overlay("factorio");
    let rendered = render_guide(
        "factorio",
        &GuideSubject::NoRegistry,
        Some(&overlay),
        CATALOG,
    );
    assert!(rendered.contains(".grizzly/env"), "{rendered}");
    assert!(rendered.contains("LOAD_LATEST_SAVE"), "{rendered}");
    assert!(rendered.contains("SAVE_NAME"), "{rendered}");
    assert!(
        rendered.contains("not stored in any config file"),
        "{rendered}"
    );
}
