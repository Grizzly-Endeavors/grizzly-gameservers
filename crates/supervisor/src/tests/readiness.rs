use std::net::Ipv4Addr;

use tokio::net::TcpListener;

use super::*;

#[tokio::test]
async fn returns_true_once_the_port_accepts_connections() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    // The listener only needs to exist for the connect to succeed; nothing has
    // to accept the connection.
    let accepted = wait_accepting(port, Duration::from_secs(5)).await;
    assert!(accepted, "a bound, listening port should be seen as ready");
}

#[tokio::test]
async fn gives_up_once_the_deadline_has_already_passed() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener); // nothing listens on `port` anymore

    let accepted = wait_accepting(port, Duration::ZERO).await;
    assert!(
        !accepted,
        "an already-past deadline should give up without a connection"
    );
}

#[test]
fn log_ready_watch_matches_only_lines_containing_the_marker() {
    let (tx, _rx) = mpsc::channel(1);
    let watch = LogReadyWatch::new(Arc::from("Game server connected"), tx);
    assert!(
        watch.matches("2026-07-08 12:00:00 Game server connected (steamid)"),
        "a line embedding the marker should match"
    );
    assert!(
        !watch.matches("DungeonDB Start 1234"),
        "an unrelated line should not match"
    );
}

#[tokio::test]
async fn log_ready_watch_signals_the_runner_once_the_marker_is_seen() {
    let (tx, mut rx) = mpsc::channel(1);
    let watch = LogReadyWatch::new(Arc::from("ready"), tx);
    assert!(watch.try_signal().is_ok(), "first signal delivers");
    assert!(
        rx.recv().await.is_some(),
        "runner receives the ready signal"
    );
}

#[test]
fn log_ready_watch_signal_is_benign_when_the_runner_is_gone() {
    let (tx, rx) = mpsc::channel(1);
    let watch = LogReadyWatch::new(Arc::from("ready"), tx);
    drop(rx); // runner has shut down
    assert!(
        watch.try_signal().is_err(),
        "signalling a closed channel reports the error rather than panicking"
    );
}
