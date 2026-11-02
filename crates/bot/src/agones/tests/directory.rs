use super::*;
use grizzly_control_api::StatusResponse;

fn server(game: &str, url: &str) -> FixedServer {
    FixedServer {
        game: game.to_owned(),
        control_url: url.to_owned(),
    }
}

fn two_servers() -> FixedServers {
    FixedServers::new([
        (
            "ev-factorio".to_owned(),
            server("factorio", "http://127.0.0.1:19359/"),
        ),
        (
            "ev-terraria".to_owned(),
            server("terraria", "http://127.0.0.1:19360"),
        ),
    ])
    .unwrap()
}

fn status(process: ProcessPhase) -> StatusPoll {
    StatusPoll::Ok(StatusResponse {
        process,
        ready: true,
        pid: None,
        uptime_seconds: 0,
        restarts: 0,
    })
}

#[test]
fn a_listed_server_resolves_to_its_control_url_without_a_trailing_slash() {
    let fixed = two_servers();
    let PodTarget::Ready(base) = fixed.target("ev-factorio") else {
        panic!("a listed server should be ready to dial");
    };
    assert_eq!(base, "http://127.0.0.1:19359");
}

#[test]
fn an_unlisted_server_is_not_found() {
    assert!(matches!(
        two_servers().target("ev-minecraft"),
        PodTarget::NotFound
    ));
}

#[test]
fn every_listed_server_is_in_scope_and_nothing_else_is() {
    let fixed = two_servers();
    assert_eq!(fixed.scope_verdict("ev-terraria"), ScopeVerdict::InScope);
    assert_eq!(fixed.scope_verdict("ev-minecraft"), ScopeVerdict::Absent);
}

#[test]
fn a_listed_server_is_owned_by_no_guild_and_runs_its_game() {
    let owner = two_servers().owner("ev-terraria").unwrap();
    assert_eq!(owner.guild, None);
    assert_eq!(owner.game, Some(GameId::new("terraria")));
    assert_eq!(two_servers().owner("ev-minecraft"), None);
}

#[test]
fn a_repeated_name_is_refused() {
    let err = FixedServers::new([
        ("ev-x".to_owned(), server("factorio", "http://127.0.0.1:1")),
        ("ev-x".to_owned(), server("factorio", "http://127.0.0.1:2")),
    ])
    .unwrap_err();
    assert!(err.contains("ev-x"), "{err}");
}

#[test]
fn a_non_http_control_url_is_refused() {
    let err = FixedServers::new([("ev-x".to_owned(), server("factorio", "https://127.0.0.1:1"))])
        .unwrap_err();
    assert!(err.contains("http://"), "{err}");
}

#[test]
fn an_empty_name_is_refused() {
    assert!(
        FixedServers::new([(" ".to_owned(), server("factorio", "http://127.0.0.1:1"))]).is_err()
    );
}

#[test]
fn listing_state_follows_the_process_phase() {
    assert_eq!(
        fixed_state(&status(ProcessPhase::Running)),
        ServerState::Ready
    );
    assert_eq!(
        fixed_state(&status(ProcessPhase::Stopped)),
        ServerState::Paused
    );
    assert_eq!(
        fixed_state(&status(ProcessPhase::Crashed)),
        ServerState::Other("Crashed".to_owned())
    );
}

#[test]
fn an_unanswered_poll_lists_as_unknown_rather_than_down() {
    assert_eq!(fixed_state(&StatusPoll::Unreachable), ServerState::Unknown);
}
