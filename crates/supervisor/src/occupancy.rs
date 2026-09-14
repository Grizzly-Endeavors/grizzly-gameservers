//! Where the live connected-player count comes from.
//!
//! Most games answer over RCON, so [`crate::rcon`] is the usual source. The games
//! without a console need another: Satisfactory exposes a dedicated-server HTTPS
//! API, Terraria's `TShock` flavor a REST API, and Enshrouded only the Steam query
//! port every Steam-listed server answers the browser on. Without a count a game
//! can never join update-on-empty — the auto-updater only bounces a build it can
//! prove nobody is playing — and Gary's check-before-restart shows `unknown`
//! rather than a number.
//!
//! Every source answers the same question the same way, including the failure mode:
//! `None` means **unknown**, never a definite zero. A caller that reads a failed
//! probe as "empty" would restart a server with people on it.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tokio::time::timeout;
use tracing::{debug, warn};

use crate::rcon::RconRuntime;

/// Which occupancy protocol a game image names. Selected per image rather than
/// sniffed: they are different protocols on different ports, and guessing wrong
/// would report every server as unknown-occupancy with no obvious cause.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OccupancyApi {
    /// Satisfactory's dedicated-server HTTPS API.
    Satisfactory,
    /// `TShock`'s REST API, for Terraria.
    Tshock,
    /// The Steam server query protocol, for Enshrouded.
    A2s,
}

impl OccupancyApi {
    /// Parse the `SUPERVISOR_OCCUPANCY_API` value a game image sets.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "satisfactory" => Some(Self::Satisfactory),
            "tshock" => Some(Self::Tshock),
            "a2s" => Some(Self::A2s),
            _ => None,
        }
    }
}

/// A fully-addressed non-RCON occupancy source, as the config layer assembles it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OccupancyEndpoint {
    /// Satisfactory's API at this base URL.
    Satisfactory(String),
    /// `TShock`'s REST API at this base URL.
    Tshock(String),
    /// The Steam query protocol on this loopback port.
    A2s(u16),
}

/// Privilege level requested from the Satisfactory API. `Client` is the lowest
/// level that can read server state, and passwordless login only succeeds at it
/// while no client-protection password is set — which is the case for a server
/// the shim provisions.
const CLIENT_PRIVILEGE: &str = "Client";

/// How the supervisor reads the current player count for this game.
pub enum OccupancySource {
    /// The game's console reports it (most games).
    Rcon(Arc<RconRuntime>),
    /// Satisfactory's dedicated-server HTTPS API reports it.
    SatisfactoryApi(Arc<SatisfactoryApi>),
    /// `TShock`'s REST API reports it, for Terraria — which has no console of its
    /// own on any flavor.
    TshockApi(Arc<TshockApi>),
    /// The Steam query port reports it, for Enshrouded — which has no console and
    /// no API of any other kind.
    A2s(Arc<A2sQuery>),
}

impl OccupancySource {
    /// The number of players currently connected, or `None` when it couldn't be
    /// determined. Never returns `Some(0)` on a failed probe.
    pub async fn player_count(&self) -> Option<u32> {
        match self {
            Self::Rcon(rcon) => rcon.player_count().await,
            Self::SatisfactoryApi(api) => api.player_count().await,
            Self::TshockApi(api) => api.player_count().await,
            Self::A2s(query) => query.player_count().await,
        }
    }
}

/// The `A2S_INFO` request: the simple-packet header, the query type, and the fixed
/// payload string the protocol requires.
const A2S_INFO_REQUEST: &[u8] = b"\xFF\xFF\xFF\xFFTSource Engine Query\0";

/// The header that opens every single-packet A2S reply.
const A2S_SIMPLE_HEADER: [u8; 4] = [0xFF; 4];

/// How long to wait for each reply. The server is on loopback, so a reply that
/// hasn't arrived in this long isn't coming — typically the world is still loading
/// and the query port isn't answering yet.
const A2S_REPLY_TIMEOUT: Duration = Duration::from_secs(2);

/// Client for the Steam server query protocol, scoped to `A2S_INFO`'s player count.
///
/// Queries the game over loopback inside this pod, on the same port the server
/// browser queries it on from outside. Unauthenticated by design: `A2S_INFO` is
/// what a Steam-listed server answers to anyone, so reading it mints and stores no
/// credential.
pub struct A2sQuery {
    server: SocketAddr,
}

/// One reply to an `A2S_INFO` request.
#[derive(Debug, PartialEq, Eq)]
enum A2sInfoReply {
    /// The server wants the request repeated with this challenge appended — the
    /// protocol's guard against reflection floods, which Enshrouded enforces.
    Challenge([u8; 4]),
    /// The server's info, reduced to the one field the supervisor reads.
    Players(u32),
}

impl A2sQuery {
    /// Build a client for the query port `port` on loopback.
    #[must_use]
    pub fn new(port: u16) -> Self {
        Self {
            server: SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        }
    }

    /// Players currently connected, or `None` when the query port couldn't be read.
    pub async fn player_count(&self) -> Option<u32> {
        match self.query_info().await {
            Ok(count) => Some(count),
            Err(err) => {
                // Warn, not error: the port stays silent while the world loads, and
                // the caller degrades to "unknown".
                warn!(error = ?err, port = self.server.port(), "could not read a2s player count");
                None
            }
        }
    }

    async fn query_info(&self) -> Result<u32> {
        let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .await
            .context("failed to bind the a2s query socket")?;
        socket
            .connect(self.server)
            .await
            .with_context(|| format!("failed to address the a2s query to {}", self.server))?;

        let mut request = A2S_INFO_REQUEST.to_vec();
        // One challenge round at most: a server that challenges a request already
        // carrying its challenge isn't going to answer a third time either.
        for _ in 0..2 {
            socket
                .send(&request)
                .await
                .context("failed to send the a2s info request")?;
            let mut buf = [0_u8; 1400];
            let len = timeout(A2S_REPLY_TIMEOUT, socket.recv(&mut buf))
                .await
                .context("the a2s query port did not reply in time")?
                .context("failed to receive the a2s reply")?;
            let reply = buf
                .get(..len)
                .context("a2s reply length exceeds the buffer")?;
            match parse_a2s_info_reply(reply)? {
                A2sInfoReply::Players(count) => return Ok(count),
                A2sInfoReply::Challenge(challenge) => {
                    request = [A2S_INFO_REQUEST, &challenge].concat();
                }
            }
        }
        bail!("the a2s query port answered a challenged request with another challenge")
    }
}

/// Parse one `A2S_INFO` reply datagram.
///
/// # Errors
///
/// Returns an error when the datagram isn't a single-packet reply, is of a type
/// other than a challenge or an info reply, or ends before the player count.
fn parse_a2s_info_reply(reply: &[u8]) -> Result<A2sInfoReply> {
    let body = reply
        .strip_prefix(&A2S_SIMPLE_HEADER)
        .context("a2s reply is not a single-packet response")?;
    let (&kind, rest) = body.split_first().context("a2s reply is empty")?;
    match kind {
        b'A' => {
            let challenge = rest
                .get(..4)
                .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
                .context("a2s challenge reply is shorter than its 4-byte challenge")?;
            Ok(A2sInfoReply::Challenge(challenge))
        }
        b'I' => {
            // A protocol-version byte, then four null-terminated strings (name,
            // map, folder, game), then a two-byte app id, then the player count.
            let mut fields = rest
                .get(1..)
                .context("a2s info reply ends before its fields")?;
            for field in ["name", "map", "folder", "game"] {
                let end = fields
                    .iter()
                    .position(|&byte| byte == 0)
                    .with_context(|| format!("a2s info reply's {field} is unterminated"))?;
                fields = fields
                    .get(end + 1..)
                    .with_context(|| format!("a2s info reply ends after its {field}"))?;
            }
            let players = fields
                .get(2)
                .copied()
                .context("a2s info reply ends before its player count")?;
            Ok(A2sInfoReply::Players(u32::from(players)))
        }
        other => bail!("unexpected a2s reply type {other:#04x}"),
    }
}

/// Client for `TShock`'s REST API, scoped to the one question the supervisor asks.
///
/// Unauthenticated on purpose: `/v2/server/status` is one of the few `TShock`
/// endpoints that takes no token, so reading occupancy needs no credential to
/// mint, store, or rotate. Everything that *does* mutate the server still
/// requires one, and the port is pod-internal regardless.
pub struct TshockApi {
    http: reqwest::Client,
    status_url: String,
}

#[derive(Deserialize)]
struct TshockStatus {
    /// `TShock` renders this as a JSON number; `status` alongside it is a
    /// *string* ("200"), which is why only this field is modelled.
    playercount: u32,
}

impl TshockApi {
    /// Build a client for the REST API at `base_url` (e.g. `http://127.0.0.1:7878`).
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client can't be constructed.
    pub fn new(base_url: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .build()
            .context("failed to build the tshock api client")?;
        Ok(Self {
            http,
            status_url: format!("{}/v2/server/status", base_url.trim_end_matches('/')),
        })
    }

    /// Players currently connected, or `None` when the API couldn't be read.
    pub async fn player_count(&self) -> Option<u32> {
        match self.query_status().await {
            Ok(count) => Some(count),
            Err(err) => {
                // Warn, not error: an unreachable API is expected while the world
                // is still generating, and the caller degrades to "unknown".
                warn!(error = ?err, "could not read tshock player count");
                None
            }
        }
    }

    async fn query_status(&self) -> Result<u32> {
        let status: TshockStatus = self
            .http
            .get(&self.status_url)
            .send()
            .await
            .context("tshock status request failed")?
            .error_for_status()
            .context("tshock status returned an error status")?
            .json()
            .await
            .context("could not parse the tshock status response")?;
        Ok(status.playercount)
    }
}

/// Client for Satisfactory's dedicated-server HTTPS API, scoped to the one
/// question the supervisor asks it.
///
/// Talks to the game over loopback inside this pod. The server generates its own
/// self-signed certificate with no CA to pin it against, so the client does not
/// verify it — there is no position from which to intercept a connection that
/// never leaves the container's network namespace, which is the same reasoning
/// that lets the Agones SDK client speak plain HTTP to `127.0.0.1:9358`.
pub struct SatisfactoryApi {
    http: reqwest::Client,
    endpoint: String,
    /// The bearer token from the last successful login. Cached because the poll
    /// runs on a cadence and re-authenticating every tick would be pure waste;
    /// cleared and re-fetched when the server stops accepting it.
    token: Mutex<Option<String>>,
}

#[derive(Deserialize)]
struct LoginEnvelope {
    data: LoginData,
}

#[derive(Deserialize)]
struct LoginData {
    #[serde(rename = "authenticationToken")]
    authentication_token: String,
}

#[derive(Deserialize)]
struct StateEnvelope {
    data: StateData,
}

#[derive(Deserialize)]
struct StateData {
    #[serde(rename = "serverGameState")]
    server_game_state: ServerGameState,
}

#[derive(Deserialize)]
struct ServerGameState {
    #[serde(rename = "numConnectedPlayers")]
    num_connected_players: u32,
}

impl SatisfactoryApi {
    /// Build a client for the API at `endpoint` (e.g.
    /// `https://127.0.0.1:7777/api/v1`).
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTPS client can't be constructed.
    pub fn new(endpoint: String) -> Result<Self> {
        // The endpoint is 127.0.0.1 inside this pod, and the certificate is one
        // the game generated for itself — there is no CA to pin it against and
        // no position from which to intercept a connection that never leaves
        // the container's network namespace. Verification would therefore
        // reject every valid certificate while excluding no attacker. Same
        // reasoning as the Agones SDK client speaking plain HTTP to 127.0.0.1.
        // The suppression must sit on the builder line: semgrep anchors the
        // finding at the expression's first line, not at the accept call.
        // nosemgrep: rust.lang.security.reqwest-accept-invalid
        let http = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .context("failed to build the satisfactory api client")?;
        Ok(Self {
            http,
            endpoint,
            token: Mutex::new(None),
        })
    }

    /// Players currently connected, or `None` when the API couldn't be read.
    ///
    /// Retries once without the cached token: the server restarts its API with the
    /// game, so a token minted before a relaunch is rejected afterwards, and a poll
    /// that gave up there would report `unknown` forever after the first restart.
    pub async fn player_count(&self) -> Option<u32> {
        match self.query_state().await {
            Ok(count) => Some(count),
            Err(first) => {
                debug!(error = ?first, "satisfactory state query failed; re-authenticating");
                self.token.lock().await.take();
                match self.query_state().await {
                    Ok(count) => Some(count),
                    Err(err) => {
                        // Warn, not error: an unreachable API is expected while the
                        // game is still booting, and the caller degrades to
                        // "unknown" rather than acting on it.
                        warn!(error = ?err, "could not read satisfactory player count");
                        None
                    }
                }
            }
        }
    }

    async fn query_state(&self) -> Result<u32> {
        let token = self.authenticated_token().await?;
        let response = self
            .http
            .post(&self.endpoint)
            .bearer_auth(token)
            .json(&serde_json::json!({ "function": "QueryServerState" }))
            .send()
            .await
            .context("satisfactory QueryServerState request failed")?
            .error_for_status()
            .context("satisfactory QueryServerState returned an error status")?;
        let envelope: StateEnvelope = response
            .json()
            .await
            .context("could not parse the satisfactory QueryServerState response")?;
        Ok(envelope.data.server_game_state.num_connected_players)
    }

    /// The cached bearer token, logging in if there isn't one.
    async fn authenticated_token(&self) -> Result<String> {
        let mut cached = self.token.lock().await;
        if let Some(token) = cached.as_ref() {
            return Ok(token.clone());
        }
        let token = self.login().await?;
        *cached = Some(token.clone());
        Ok(token)
    }

    async fn login(&self) -> Result<String> {
        let response = self
            .http
            .post(&self.endpoint)
            .json(&serde_json::json!({
                "function": "PasswordlessLogin",
                "data": { "MinimumPrivilegeLevel": CLIENT_PRIVILEGE },
            }))
            .send()
            .await
            .context("satisfactory PasswordlessLogin request failed")?
            .error_for_status()
            .context("satisfactory PasswordlessLogin returned an error status")?;
        let envelope: LoginEnvelope = response
            .json()
            .await
            .context("could not parse the satisfactory PasswordlessLogin response")?;
        Ok(envelope.data.authentication_token)
    }
}

#[cfg(test)]
#[path = "tests/occupancy.rs"]
mod tests;
