use std::collections::HashSet;

use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::Service;
use kube::api::ListParams;
use kube::{Api, Client};
use tracing::warn;

use grizzly_control_api::{PROCESS_LABEL_KEY, PROCESS_LABEL_STOPPED};

use super::labels::{GAME_KEY, GUILD_KEY, is_managed, label_value, service_gameserver_target};
use super::ports::{friend_facing_node_port, node_ports_by_gameserver};
use super::provision::{DeletionState, deletion_state};
use super::scope::ServerScope;
use super::types::{GameServer, ServerState, ServerSummary, summarize};
use crate::domain::{GameId, GuildId, InstanceName};

/// List the Agones `GameServer`s in `namespace` visible under `scope`, joining
/// each to its `NodePort` Service to resolve a connection address under
/// `domain`. A guild scope filters both lists to that guild's servers via
/// the [`GUILD_KEY`](super::labels::GUILD_KEY) label; the operator scope
/// lists everything.
///
/// # Errors
///
/// Returns an error if the gameservers or services cannot be listed from the
/// Kubernetes API.
pub(crate) async fn list_active_servers(
    client: Client,
    namespace: &str,
    domain: &str,
    scope: &ServerScope,
) -> Result<Vec<ServerSummary>> {
    let gameservers: Api<GameServer> = Api::namespaced(client.clone(), namespace);
    let services: Api<Service> = Api::namespaced(client, namespace);

    let mut params = ListParams::default();
    if let Some(selector) = scope.label_selector() {
        params = params.labels(&selector);
    }

    let gs_list = gameservers
        .list(&params)
        .await
        .with_context(|| format!("failed to list gameservers in namespace {namespace}"))?;
    let svc_list = services
        .list(&params)
        .await
        .with_context(|| format!("failed to list services in namespace {namespace}"))?;

    let node_port_by_server = node_ports_by_gameserver(&svc_list.items);

    let mut summaries = Vec::with_capacity(gs_list.items.len());
    let mut live: HashSet<&str> = HashSet::new();
    for gameserver in &gs_list.items {
        // Only shim-provisioned instances are the bot's domain — skip Flux-managed
        // singletons so the live half matches the stopped half and the backup
        // targets, all of which are managed-only.
        if !is_managed(gameserver.metadata.labels.as_ref()) {
            continue;
        }
        let instance = gameserver.metadata.name.as_deref().unwrap_or("<unnamed>");
        live.insert(instance);
        // The supervisor publishes a paused process as a GameServer label, which
        // takes precedence over the (still Ready/Allocated) Agones state.
        let paused = label_value(gameserver.metadata.labels.as_ref(), PROCESS_LABEL_KEY)
            == Some(PROCESS_LABEL_STOPPED);
        // A GameServer being torn down keeps reporting its last Agones state
        // until the finalizer clears, so listing that state would show a server
        // as Ready while it is on its way out — and, if its node is gone, keep
        // showing it indefinitely.
        let state = if deletion_state(&gameserver.metadata) == DeletionState::Terminating {
            ServerState::Stopping
        } else if paused {
            ServerState::Paused
        } else {
            gameserver
                .status
                .as_ref()
                .and_then(|status| status.state.as_deref())
                .map_or(ServerState::Unknown, ServerState::from_agones)
        };
        let node_port = node_port_by_server.get(instance).copied();
        if node_port.is_none() {
            warn!(
                gameserver = instance,
                "no NodePort service matched gameserver; address omitted"
            );
        }
        let game = label_value(gameserver.metadata.labels.as_ref(), GAME_KEY);
        summaries.push(summarize(instance, game, state, node_port, domain));
    }

    append_stopped_instances(&svc_list.items, &live, domain, &mut summaries);
    Ok(summaries)
}

/// The Agones state string that marks a `GameServer` whose health heartbeat
/// stopped — for this bot's supervisors, crash escalation (ADR-010). The pod is
/// torn down shortly after, but the `GameServer` object persists in this state.
const UNHEALTHY_STATE: &str = "Unhealthy";

/// A managed server whose `GameServer` object is live: who owns it, what it
/// runs, whether its game process is up, and whether Agones has declared it
/// unhealthy. The pod usually exists too, but an unhealthy instance may have
/// already lost it.
///
/// The `running` flag carries different weight per consumer. The backup cycle
/// uses it to decide whether to quiesce (flush) before snapshotting — a paused
/// server's `/data` is already saved and its RCON is down, so quiescing would
/// only log a spurious failure. The occupancy sampler uses it to gate the
/// occupancy read and the sample row. Note that crash escalation also labels
/// the process `stopped` (the supervisor's runner does this on giving up), so
/// `running: false` alone cannot distinguish "paused on purpose" from
/// "escalated crash" — that's what `unhealthy` is for.
pub(crate) struct LiveInstance {
    pub(crate) instance: InstanceName,
    pub(crate) game: GameId,
    pub(crate) guild: GuildId,
    pub(crate) running: bool,
    pub(crate) unhealthy: bool,
}

/// The managed `GameServer`s with a live pod, for the consumers that need to
/// reach a supervisor: the scheduled backup cycle and the occupancy sampler.
/// Shut-down instances (Service only, no pod) are absent — there is nothing in
/// the cluster to talk to — and are recovered via archive, not backed up.
///
/// This is the one listing that surfaces the owning guild, which the labels
/// carry but [`list_active_servers`]'s friend-facing summary does not.
///
/// # Errors
///
/// Returns an error if the gameservers can't be listed from the Kubernetes API.
pub(crate) async fn list_live_instances(
    client: &Client,
    namespace: &str,
) -> Result<Vec<LiveInstance>> {
    let gameservers: Api<GameServer> = Api::namespaced(client.clone(), namespace);
    let list = gameservers
        .list(&ListParams::default())
        .await
        .with_context(|| format!("failed to list gameservers in namespace {namespace}"))?;
    let mut targets = Vec::new();
    for gameserver in &list.items {
        let labels = gameserver.metadata.labels.as_ref();
        if !is_managed(labels) {
            continue;
        }
        let Some(instance) = gameserver.metadata.name.clone() else {
            continue;
        };
        let running = label_value(labels, PROCESS_LABEL_KEY) != Some(PROCESS_LABEL_STOPPED);
        let unhealthy = gameserver
            .status
            .as_ref()
            .and_then(|status| status.state.as_deref())
            == Some(UNHEALTHY_STATE);
        targets.push(LiveInstance {
            instance: InstanceName::new(instance),
            game: GameId::new(label_value(labels, GAME_KEY).unwrap_or_default()),
            guild: GuildId::new(label_value(labels, GUILD_KEY).unwrap_or_default()),
            running,
            unhealthy,
        });
    }
    Ok(targets)
}

/// A `/shutdown` deletes the `GameServer` but keeps its Service, so a managed Service
/// with no live `GameServer` behind it is a stopped instance. Surface those so a
/// friend can see — and `/start` — a world that is currently down.
fn append_stopped_instances(
    services: &[Service],
    live: &HashSet<&str>,
    domain: &str,
    summaries: &mut Vec<ServerSummary>,
) {
    for service in services {
        if !is_managed(service.metadata.labels.as_ref()) {
            continue;
        }
        let Some(target) = service_gameserver_target(service) else {
            continue;
        };
        if live.contains(target) {
            continue;
        }
        let node_port = friend_facing_node_port(service);
        let game = label_value(service.metadata.labels.as_ref(), GAME_KEY);
        summaries.push(summarize(
            target,
            game,
            ServerState::Stopped,
            node_port,
            domain,
        ));
    }
}
