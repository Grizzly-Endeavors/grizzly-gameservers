#![expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]

use std::path::PathBuf;

use super::*;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src/agones/tests/fixtures")
        .join(name)
}

#[tokio::test]
async fn loads_game_dirs_and_skips_underscore_prefixed() {
    let catalog = load_catalog(&fixture("catalog")).await.unwrap();
    let ids: Vec<&str> = catalog.game_ids().collect();
    assert_eq!(ids, vec!["testgame"], "_template must be skipped");

    let entry = catalog.get("testgame").unwrap();
    assert!(entry.gameserver_yaml.contains("kind: GameServer"));
    assert!(entry.service_yaml.contains("kind: Service"));
    assert!(entry.pvc_yaml.contains("kind: PersistentVolumeClaim"));
}

#[tokio::test]
async fn missing_template_names_the_path() {
    let err = load_catalog(&fixture("catalog-missing")).await.unwrap_err();
    let message = format!("{err:#}");
    assert!(
        message.contains("service.yaml"),
        "error should name the missing template, got: {message}"
    );
}

#[tokio::test]
async fn missing_directory_is_an_error() {
    let err = load_catalog(&fixture("does-not-exist")).await.unwrap_err();
    assert!(
        format!("{err:#}").contains("failed to read catalog directory"),
        "error should explain the directory could not be read, got: {err:#}"
    );
}

#[test]
fn valid_game_id_accepts_lowercase_label() {
    assert!(validate_game_id("minecraft").is_ok());
    assert!(validate_game_id("valheim-2").is_ok());
}

#[test]
fn valid_game_id_rejects_uppercase_and_edge_dashes() {
    assert!(validate_game_id("Minecraft").is_err());
    assert!(validate_game_id("-mc").is_err());
    assert!(validate_game_id("mc-").is_err());
    assert!(validate_game_id("").is_err());
}
