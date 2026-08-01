use super::*;

/// The Minecraft catalog's real allowlist, so the tests exercise the shipped
/// policy rather than a convenient fiction.
fn minecraft_allowlist() -> Vec<String> {
    [
        "TYPE",
        "VERSION",
        "MODRINTH_PROJECTS",
        "MODRINTH_DOWNLOAD_DEPENDENCIES",
        "MODRINTH_PROJECTS_DEFAULT_VERSION_TYPE",
    ]
    .iter()
    .map(|key| (*key).to_owned())
    .collect()
}

fn applied(text: &str) -> Vec<(String, String)> {
    resolve(text, &minecraft_allowlist(), "RCON_PASSWORD").applied
}

#[test]
fn empty_input_yields_nothing() {
    let resolution = resolve("", &minecraft_allowlist(), "RCON_PASSWORD");
    assert!(resolution.applied.is_empty(), "no keys from an empty file");
    assert!(resolution.skipped.is_empty(), "and nothing to warn about");
}

#[test]
fn ignores_comments_and_blank_lines() {
    let text = "# mods for the survival world\n\nTYPE=FABRIC\n   \n   # indented comment\n";
    let resolution = resolve(text, &minecraft_allowlist(), "RCON_PASSWORD");
    assert_eq!(
        resolution.applied,
        vec![("TYPE".to_owned(), "FABRIC".to_owned())],
        "only the real assignment survives"
    );
    assert!(
        resolution.skipped.is_empty(),
        "comments and blanks are not skips: {:?}",
        resolution.skipped
    );
}

#[test]
fn trims_surrounding_whitespace_from_key_and_value() {
    assert_eq!(
        applied("   TYPE   =   FABRIC   \n"),
        vec![("TYPE".to_owned(), "FABRIC".to_owned())]
    );
}

#[test]
fn value_keeps_everything_after_the_first_equals() {
    // A version-pinned Modrinth entry is the realistic case for an embedded `=`.
    assert_eq!(
        applied("MODRINTH_PROJECTS=fabric-api:0.1=2\n"),
        vec![(
            "MODRINTH_PROJECTS".to_owned(),
            "fabric-api:0.1=2".to_owned()
        )]
    );
}

#[test]
fn quotes_are_literal_not_stripped() {
    // No dotenv quoting semantics: itzg would receive the quotes as part of the
    // slug list and fail to resolve it, which is the behavior we want to be
    // obvious rather than silently "helpful".
    assert_eq!(
        applied("TYPE=\"FABRIC\"\n"),
        vec![("TYPE".to_owned(), "\"FABRIC\"".to_owned())]
    );
}

#[test]
fn malformed_line_is_skipped_and_the_rest_still_applies() {
    let resolution = resolve(
        "this is not an assignment\nTYPE=FABRIC\n",
        &minecraft_allowlist(),
        "RCON_PASSWORD",
    );
    assert_eq!(
        resolution.applied,
        vec![("TYPE".to_owned(), "FABRIC".to_owned())],
        "a broken line must not cost the good ones"
    );
    assert_eq!(
        resolution.skipped,
        vec![OverlaySkip::Malformed { line_number: 1 }]
    );
}

#[test]
fn empty_key_is_malformed() {
    let resolution = resolve("=FABRIC\n", &minecraft_allowlist(), "RCON_PASSWORD");
    assert!(resolution.applied.is_empty());
    assert_eq!(
        resolution.skipped,
        vec![OverlaySkip::Malformed { line_number: 1 }]
    );
}

#[test]
fn duplicate_key_takes_the_last_occurrence() {
    let resolution = resolve(
        "TYPE=VANILLA\nVERSION=1.21\nTYPE=FABRIC\n",
        &minecraft_allowlist(),
        "RCON_PASSWORD",
    );
    assert_eq!(
        resolution.applied,
        vec![
            ("VERSION".to_owned(), "1.21".to_owned()),
            ("TYPE".to_owned(), "FABRIC".to_owned()),
        ],
        "last wins, and the superseded entry leaves no stale slot"
    );
    assert_eq!(
        resolution.skipped,
        vec![OverlaySkip::Duplicate {
            key: "TYPE".to_owned()
        }]
    );
}

#[test]
fn key_matching_is_case_sensitive() {
    let resolution = resolve("type=FABRIC\n", &minecraft_allowlist(), "RCON_PASSWORD");
    assert!(
        resolution.applied.is_empty(),
        "a mis-cased key misses the allowlist rather than being normalized"
    );
    assert_eq!(
        resolution.skipped,
        vec![OverlaySkip::NotAllowlisted {
            key: "type".to_owned()
        }]
    );
}

#[test]
fn non_allowlisted_key_is_skipped() {
    let resolution = resolve("MEMORY=8G\n", &minecraft_allowlist(), "RCON_PASSWORD");
    assert!(resolution.applied.is_empty());
    assert_eq!(
        resolution.skipped,
        vec![OverlaySkip::NotAllowlisted {
            key: "MEMORY".to_owned()
        }]
    );
}

#[test]
fn supervisor_prefixed_keys_are_denied_even_when_allowlisted() {
    // The catalog-error case: honoring this would let an instance rewrite the
    // very allowlist that bounds it.
    let allowlist = vec!["SUPERVISOR_CHILD_ENV_ALLOWLIST".to_owned()];
    let resolution = resolve(
        "SUPERVISOR_CHILD_ENV_ALLOWLIST=MEMORY\n",
        &allowlist,
        "RCON_PASSWORD",
    );
    assert!(resolution.applied.is_empty(), "reserved prefix always wins");
    assert_eq!(
        resolution.skipped,
        vec![OverlaySkip::Reserved {
            key: "SUPERVISOR_CHILD_ENV_ALLOWLIST".to_owned(),
            allowlisted: true,
        }]
    );
}

#[test]
fn supervisor_prefixed_key_is_denied_without_being_allowlisted() {
    let resolution = resolve(
        "SUPERVISOR_DATA_DIR=/etc\n",
        &minecraft_allowlist(),
        "RCON_PASSWORD",
    );
    assert!(resolution.applied.is_empty());
    assert_eq!(
        resolution.skipped,
        vec![OverlaySkip::Reserved {
            key: "SUPERVISOR_DATA_DIR".to_owned(),
            allowlisted: false,
        }],
        "not a catalog error — the catalog never listed it"
    );
}

#[test]
fn rcon_password_is_denied_under_its_resolved_name() {
    // Palworld renames it, so a hardcoded "RCON_PASSWORD" check would let the
    // overlay hijack the password the supervisor authenticates with. Deliberately
    // allowlisted here so this fails if the deny is ever moved behind the
    // allowlist check.
    let allowlist = vec!["ADMIN_PASSWORD".to_owned(), "TYPE".to_owned()];
    let resolution = resolve(
        "ADMIN_PASSWORD=hunter2\nTYPE=FABRIC\n",
        &allowlist,
        "ADMIN_PASSWORD",
    );
    assert_eq!(
        resolution.applied,
        vec![("TYPE".to_owned(), "FABRIC".to_owned())]
    );
    assert_eq!(
        resolution.skipped,
        vec![OverlaySkip::PlatformOwned {
            key: "ADMIN_PASSWORD".to_owned(),
            allowlisted: true,
        }]
    );
}

#[test]
fn a_renamed_rcon_env_leaves_the_default_name_merely_unallowlisted() {
    // The mirror of the test above: with the password living under
    // ADMIN_PASSWORD, plain RCON_PASSWORD is just another key the game doesn't
    // expose — proving the deny tracks the resolved name, not both.
    let resolution = resolve("RCON_PASSWORD=hunter2\n", &[], "ADMIN_PASSWORD");
    assert!(resolution.applied.is_empty());
    assert_eq!(
        resolution.skipped,
        vec![OverlaySkip::NotAllowlisted {
            key: "RCON_PASSWORD".to_owned()
        }]
    );
}

#[test]
fn empty_allowlist_fails_closed() {
    let resolution = resolve("TYPE=FABRIC\nVERSION=1.21\n", &[], "RCON_PASSWORD");
    assert!(
        resolution.applied.is_empty(),
        "no allowlist means no overrides, not all overrides"
    );
    assert_eq!(resolution.skipped.len(), 2, "and every key is reported");
}

#[test]
fn applied_entries_keep_file_order_and_exact_values() {
    let text = "MODRINTH_PROJECTS=datapack:terralith,fabric-api\n\
                MODRINTH_DOWNLOAD_DEPENDENCIES=required\n\
                TYPE=FABRIC\n";
    assert_eq!(
        applied(text),
        vec![
            (
                "MODRINTH_PROJECTS".to_owned(),
                "datapack:terralith,fabric-api".to_owned()
            ),
            (
                "MODRINTH_DOWNLOAD_DEPENDENCIES".to_owned(),
                "required".to_owned()
            ),
            ("TYPE".to_owned(), "FABRIC".to_owned()),
        ]
    );
}

#[test]
fn seeding_creates_the_file_and_never_clobbers_it() {
    let root = tempfile::tempdir().unwrap();
    let path = seed(root.path(), ".grizzly/env").unwrap();
    assert_eq!(path, root.path().join(".grizzly/env"));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "",
        "seeded empty so the first real edit is an overwrite (and gets a snapshot)"
    );

    std::fs::write(&path, "TYPE=FABRIC\n").unwrap();
    seed(root.path(), ".grizzly/env").unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "TYPE=FABRIC\n",
        "re-seeding before a relaunch must not truncate an authored overlay"
    );
}

#[test]
fn seeding_rejects_a_path_outside_the_data_root() {
    let root = tempfile::tempdir().unwrap();
    assert!(
        seed(root.path(), "../escape/env").is_err(),
        "a catalog typo must not create files outside the instance volume"
    );
}
