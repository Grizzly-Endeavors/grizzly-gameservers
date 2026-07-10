use super::*;
use crate::agones::ServerState;

fn summary(name: &str, game: Option<&str>, state: &str, address: Option<&str>) -> ServerSummary {
    ServerSummary {
        name: name.to_owned(),
        game: game.map(str::to_owned),
        state: ServerState::from_agones(state),
        address: address.map(str::to_owned),
    }
}

#[test]
fn ingame_summary_is_terse_positional() {
    let server = summary("survival", Some("minecraft"), "Ready", Some("1.2.3.4:7000"));
    assert_eq!(
        format_summary(GarySurface::InGame, &server),
        "survival (minecraft, Ready, 1.2.3.4:7000)"
    );
}

#[test]
fn discord_summary_labels_each_field() {
    let server = summary("survival", Some("minecraft"), "Ready", Some("1.2.3.4:7000"));
    assert_eq!(
        format_summary(GarySurface::Discord, &server),
        "survival (game: minecraft, state: Ready, address: 1.2.3.4:7000)"
    );
}

#[test]
fn missing_game_and_address_fall_back() {
    let server = summary("world", None, "Scheduled", None);
    assert_eq!(
        format_summary(GarySurface::InGame, &server),
        "world (unknown game, Scheduled, no address yet)"
    );
}

#[test]
fn list_uses_surface_specific_separator() {
    let servers = [
        summary("a", Some("minecraft"), "Ready", Some("1.1.1.1:1")),
        summary("b", Some("valheim"), "Ready", Some("2.2.2.2:2")),
    ];
    assert!(
        format_server_list(GarySurface::InGame, &servers).contains("); "),
        "in-game list joins on a single line"
    );
    assert!(
        format_server_list(GarySurface::Discord, &servers).contains(")\n"),
        "discord list joins on newlines"
    );
}

#[test]
fn empty_list_is_one_shared_message() {
    assert_eq!(
        format_server_list(GarySurface::InGame, &[]),
        format_server_list(GarySurface::Discord, &[]),
        "the empty-list copy should not differ by surface"
    );
}

#[test]
fn no_such_carries_the_relist_hint() {
    assert_eq!(
        no_such("ghost"),
        "there's no server named ghost — check list_servers for the current names"
    );
}
