//! Where the bot finds the servers it operates, and the address each one's
//! supervisor answers on.
//!
//! Two answers exist. The deployed bot asks the cluster: a server is an Agones
//! `GameServer` in its namespace, located through the Kubernetes API. The
//! `gary-local` runner names a fixed set instead — containers started by hand,
//! each at a known control URL — so a Gary turn can run with no cluster at all.
//! Every supervisor call takes a [`Servers`] view, which is what keeps the two
//! from diverging anywhere above this module.

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Result;
use grizzly_control_api::ProcessPhase;
use kube::Client;

use super::client::list_active_servers;
use super::control_client::ControlClient;
use super::scope::{InstanceOwner, ScopeVerdict, ServerScope};
use super::supervisor::{PodTarget, StatusPoll, supervisor_status};
use super::types::{ServerState, ServerSummary};
use crate::domain::GameId;

/// One server the fixed directory knows: the catalog game it runs and the base
/// URL of its supervisor's control API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FixedServer {
    pub(crate) game: String,
    pub(crate) control_url: String,
}

/// A fixed set of servers, keyed by instance name.
#[derive(Debug, Default)]
pub(crate) struct FixedServers {
    servers: BTreeMap<String, FixedServer>,
}

impl FixedServers {
    /// Build the set from `(name, server)` pairs, rejecting a repeated name, an
    /// empty one, or a control URL that isn't plain `http://` — the supervisor
    /// serves nothing else, so anything else would only fail on first use.
    ///
    /// # Errors
    ///
    /// Returns a plain-language reason naming the offending entry.
    pub(crate) fn new(
        entries: impl IntoIterator<Item = (String, FixedServer)>,
    ) -> Result<Self, String> {
        let mut servers = BTreeMap::new();
        for (name, mut server) in entries {
            if name.trim().is_empty() {
                return Err("a server has an empty name".to_owned());
            }
            if !server.control_url.starts_with("http://") {
                return Err(format!(
                    "server {name}'s control_url must start with http://, got {:?}",
                    server.control_url
                ));
            }
            let trimmed = server.control_url.trim_end_matches('/').len();
            server.control_url.truncate(trimmed);
            if servers.insert(name.clone(), server).is_some() {
                return Err(format!("server {name} is listed twice"));
            }
        }
        Ok(Self { servers })
    }

    /// The catalog game of every listed server.
    pub(crate) fn games(&self) -> impl Iterator<Item = &str> {
        self.servers.values().map(|server| server.game.as_str())
    }

    /// Where `instance`'s control API is. Every listed server counts as managed
    /// and up: whether its process is actually running is the supervisor's to
    /// say, and an unreachable one surfaces as such on the request itself.
    fn target(&self, instance: &str) -> PodTarget {
        self.servers
            .get(instance)
            .map_or(PodTarget::NotFound, |server| {
                PodTarget::Ready(server.control_url.clone())
            })
    }

    /// The fixed set has no tenants: a listed server is in every scope.
    fn scope_verdict(&self, instance: &str) -> ScopeVerdict {
        if self.servers.contains_key(instance) {
            ScopeVerdict::InScope
        } else {
            ScopeVerdict::Absent
        }
    }

    fn owner(&self, instance: &str) -> Option<InstanceOwner> {
        self.servers.get(instance).map(|server| InstanceOwner {
            guild: None,
            game: Some(GameId::new(server.game.clone())),
        })
    }
}

/// Where the servers a [`crate::discord::Data`] operates live. Held on `Data`
/// and turned into a borrowed [`Servers`] view at each call.
#[derive(Clone, Debug)]
pub(crate) enum ServerDirectory {
    /// Agones `GameServer`s in the bot's namespace.
    Cluster,
    /// A fixed set reached directly, with no cluster behind it.
    Fixed(Arc<FixedServers>),
}

/// A borrowed view of where servers are, passed to every supervisor call.
#[derive(Clone, Copy)]
pub(crate) enum Servers<'a> {
    /// Locate each server through the Kubernetes API, then dial its pod IP on
    /// the shared control port.
    Cluster {
        client: &'a Client,
        namespace: &'a str,
        control_port: u16,
    },
    /// Dial each server at its listed control URL.
    Fixed(&'a FixedServers),
}

impl<'a> Servers<'a> {
    /// The cluster directory: servers located through `client` in `namespace`,
    /// with their supervisors dialed on `control_port`.
    pub(crate) fn cluster(client: &'a Client, namespace: &'a str, control_port: u16) -> Self {
        Self::Cluster {
            client,
            namespace,
            control_port,
        }
    }
}

impl Servers<'_> {
    /// Where `instance`'s control API is, or why it can't be reached.
    ///
    /// # Errors
    ///
    /// Returns an error if the cluster can't be queried for the gameserver or pod.
    pub(crate) async fn locate(self, instance: &str) -> Result<PodTarget> {
        match self {
            Self::Cluster {
                client,
                namespace,
                control_port,
            } => {
                super::supervisor::locate_in_cluster(client, namespace, instance, control_port)
                    .await
            }
            Self::Fixed(fixed) => Ok(fixed.target(instance)),
        }
    }

    /// Whether `instance` is visible to `scope`.
    ///
    /// # Errors
    ///
    /// Returns an error if the cluster can't be queried for the instance's Service.
    pub(crate) async fn verify_scope(
        self,
        instance: &str,
        scope: &ServerScope,
    ) -> Result<ScopeVerdict> {
        match self {
            Self::Cluster {
                client, namespace, ..
            } => super::scope::verify_scope_in_cluster(client, namespace, instance, scope).await,
            Self::Fixed(fixed) => Ok(fixed.scope_verdict(instance)),
        }
    }

    /// Which guild owns `instance` and which game it runs, or `None` when there
    /// is no such server.
    ///
    /// # Errors
    ///
    /// Returns an error if the cluster can't be queried for the instance's Service.
    pub(crate) async fn owner_of(self, instance: &str) -> Result<Option<InstanceOwner>> {
        match self {
            Self::Cluster {
                client, namespace, ..
            } => super::scope::owner_in_cluster(client, namespace, instance).await,
            Self::Fixed(fixed) => Ok(fixed.owner(instance)),
        }
    }

    /// The servers visible to `scope`, summarized for Gary. A fixed server's
    /// state is read off its supervisor, since there is no Agones state to show.
    ///
    /// # Errors
    ///
    /// Returns an error if the cluster can't be listed.
    pub(crate) async fn list(
        self,
        http: &ControlClient,
        domain: &str,
        scope: &ServerScope,
    ) -> Result<Vec<ServerSummary>> {
        match self {
            Self::Cluster {
                client, namespace, ..
            } => list_active_servers(client.clone(), namespace, domain, scope).await,
            Self::Fixed(fixed) => {
                let mut summaries = Vec::with_capacity(fixed.servers.len());
                for (name, server) in &fixed.servers {
                    let poll = supervisor_status(self, http, name).await?;
                    summaries.push(ServerSummary {
                        name: name.clone(),
                        game: Some(server.game.clone()),
                        state: fixed_state(&poll),
                        address: None,
                    });
                }
                Ok(summaries)
            }
        }
    }
}

/// A fixed server's listing state, from one status poll. `Ready` and `Paused`
/// mean what they mean on the cluster; a process in transition or crashed shows
/// its phase verbatim, and an unanswered poll is `Unknown`, never "down".
fn fixed_state(poll: &StatusPoll) -> ServerState {
    match poll {
        StatusPoll::Ok(status) => match status.process {
            ProcessPhase::Running => ServerState::Ready,
            ProcessPhase::Stopped => ServerState::Paused,
            ProcessPhase::Starting => ServerState::Other("Starting".to_owned()),
            ProcessPhase::Stopping => ServerState::Other("Stopping".to_owned()),
            ProcessPhase::Crashed => ServerState::Other("Crashed".to_owned()),
        },
        StatusPoll::NotFound
        | StatusPoll::NotManaged
        | StatusPoll::PodNotReady
        | StatusPoll::Unreachable => ServerState::Unknown,
    }
}

#[cfg(test)]
#[path = "tests/directory.rs"]
mod tests;
