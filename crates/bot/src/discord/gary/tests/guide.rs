use super::*;

const CATALOG: &str = "factorio, minecraft, valheim";

fn thunderstore() -> ModRegistry {
    ModRegistry::Thunderstore {
        community: "valheim".to_owned(),
        provided: vec!["denikson-BepInExPack_Valheim".to_owned()],
    }
}

#[test]
fn a_modrinth_guide_names_the_game_and_the_file_installs_go_through() {
    let registry = ModRegistry::Modrinth;
    let rendered = render_guide("minecraft", &GuideSubject::Registry(&registry), CATALOG);
    assert!(rendered.contains("minecraft"), "{rendered}");
    assert!(rendered.contains(".grizzly/env"), "{rendered}");
    assert!(rendered.contains("MODRINTH_PROJECTS"), "{rendered}");
}

#[test]
fn a_thunderstore_guide_teaches_pinning_and_delete_the_line() {
    // The two rules a model can't infer: a search result carries no version, and
    // removal here is the exact opposite of Minecraft's rule.
    let registry = thunderstore();
    let rendered = render_guide("valheim", &GuideSubject::Registry(&registry), CATALOG);
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
    let one = render_guide("minecraft", &GuideSubject::Registry(&modrinth), CATALOG);
    let other = render_guide("valheim", &GuideSubject::Registry(&thunder), CATALOG);
    assert!(!one.contains(".grizzly/mods"), "{one}");
    assert!(!other.contains("MODRINTH_PROJECTS"), "{other}");
}

#[test]
fn a_guide_reads_as_intro_then_choice_then_install() {
    let registry = thunderstore();
    let rendered = render_guide("valheim", &GuideSubject::Registry(&registry), CATALOG);
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
    let rendered = render_guide("factorio", &GuideSubject::NoRegistry, CATALOG);
    assert!(rendered.contains("factorio"), "{rendered}");
    assert!(rendered.contains("no supported way"), "{rendered}");
    assert!(rendered.contains("Don't improvise"), "{rendered}");
    assert!(!rendered.contains("search_mods"), "{rendered}");
}

#[test]
fn an_unknown_game_names_the_catalog_and_how_to_find_the_right_id() {
    let rendered = render_guide("valhiem", &GuideSubject::UnknownGame, CATALOG);
    assert!(rendered.contains("valhiem"), "{rendered}");
    assert!(rendered.contains(CATALOG), "{rendered}");
    assert!(rendered.contains("about_server"), "{rendered}");
}

#[test]
fn an_empty_catalog_renders_as_words_rather_than_a_blank() {
    let rendered = render_guide("anything", &GuideSubject::UnknownGame, "(none)");
    assert!(rendered.contains("(none)"), "{rendered}");
}

#[test]
fn a_thunderstore_guide_carries_the_profile_code_workflow() {
    // The fast path in both directions, and the one thing a code cannot do —
    // follow the pack after it changes.
    let guide = render_guide("valheim", &GuideSubject::Registry(&thunderstore()), CATALOG);
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
        CATALOG,
    );
    assert!(!guide.contains("profile code"), "{guide}");
}
