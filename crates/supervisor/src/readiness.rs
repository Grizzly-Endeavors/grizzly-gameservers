use std::net::Ipv4Addr;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::time::{Instant, sleep};

/// Poll interval between connection attempts while waiting for the game to bind.
const PROBE_INTERVAL: Duration = Duration::from_secs(2);

/// Wait until the game process is accepting TCP connections on `port`, giving up
/// after `give_up`. Used to gate the one-shot Agones `/ready` call: the server is
/// only "ready" once a client could actually connect.
///
/// Returns `true` once a connection succeeds, `false` if `give_up` elapses first
/// (e.g. first-boot world generation overran the window).
pub async fn wait_accepting(port: u16, give_up: Duration) -> bool {
    let deadline = Instant::now() + give_up;
    loop {
        if TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .is_ok()
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(PROBE_INTERVAL).await;
    }
}
