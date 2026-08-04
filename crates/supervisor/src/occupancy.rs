//! Where the live connected-player count comes from.
//!
//! Most games answer over RCON, so [`crate::rcon`] is the usual source. Satisfactory
//! has no RCON at all — it exposes a dedicated-server HTTPS API instead — and
//! without a count it can never join update-on-empty: the auto-updater only bounces
//! a build it can prove nobody is playing, and Gary's check-before-restart shows
//! `unknown` rather than a number.
//!
//! Both sources answer the same question the same way, including the failure mode:
//! `None` means **unknown**, never a definite zero. A caller that reads a failed
//! probe as "empty" would restart a server with people on it.

use std::sync::Arc;

use anyhow::{Context, Result};
use serde::Deserialize;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::rcon::RconRuntime;

/// Which occupancy API a game's URL speaks. Selected per image rather than
/// sniffed: the two are different protocols at different paths, and guessing
/// wrong would report every server as unknown-occupancy with no obvious cause.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OccupancyApi {
    /// Satisfactory's dedicated-server HTTPS API.
    #[default]
    Satisfactory,
    /// `TShock`'s REST API, for Terraria.
    Tshock,
}

impl OccupancyApi {
    /// Parse the `SUPERVISOR_OCCUPANCY_API` value a game image sets.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "satisfactory" => Some(Self::Satisfactory),
            "tshock" => Some(Self::Tshock),
            _ => None,
        }
    }
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
}

impl OccupancySource {
    /// The number of players currently connected, or `None` when it couldn't be
    /// determined. Never returns `Some(0)` on a failed probe.
    pub async fn player_count(&self) -> Option<u32> {
        match self {
            Self::Rcon(rcon) => rcon.player_count().await,
            Self::SatisfactoryApi(api) => api.player_count().await,
            Self::TshockApi(api) => api.player_count().await,
        }
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
        let http = reqwest::Client::builder()
            // See the type's doc comment: loopback, self-signed, no CA to pin.
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
