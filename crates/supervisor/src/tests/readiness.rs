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
