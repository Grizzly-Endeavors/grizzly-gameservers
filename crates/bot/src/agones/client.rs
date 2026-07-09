use std::collections::HashSet;

use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::Service;
use kube::api::ListParams;
use kube::{Api, Client};
use tracing::warn;

use grizzly_control_api::{PROCESS_LABEL_KEY, PROCESS_LABEL_STOPPED};

use super::labels::{GAME_KEY, GUILD_KEY, is_managed, label_value, service_gameserver_target};
use super::ports::{friend_facing_node_port, node_ports_by_gameserver};
use super::scope::ServerScope;
use super::types::{GameServer, ServerSummary, summarize};

/// State label shown for a managed instance whose Service (and leased port)
/// still exist but whose `GameServer` has been torn down by `/shutdown`.
const STOPPED_STATE: &str = "Stopped";

/// State shown for a server whose pod is up but whose game process the
/// supervisor has paused (`/stop`). Distinct from `Stopped` (= shut down).
const PAUSED_STATE: &str = "Paused";

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
        let agones_state = gameserver
            .status
            .as_ref()
            .and_then(|status| status.state.as_deref());
        // The supervisor publishes a paused process as a GameServer label, which
        // takes precedence over the (still Ready/Allocated) Agones state.
        let paused = label_value(gameserver.metadata.labels.as_ref(), PROCESS_LABEL_KEY)
            == Some(PROCESS_LABEL_STOPPED);
        let state = if paused {
            Some(PAUSED_STATE)
        } else {
            agones_state
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

/// A managed server the scheduled backup cycle should snapshot: it has a live
/// `GameServer` (pod up), so its supervisor is reachable to stream `/data`.
pub(crate) struct BackupTarget {
    pub(crate) instance: String,
    pub(crate) game: String,
    pub(crate) guild: String,
    /// Whether the game process is up — drives whether the snapshot quiesces
    /// (flushes) first. A paused server's `/data` is already saved, and its RCON
    /// is down, so quiescing it would only log a spurious failure.
    pub(crate) running: bool,
}

/// The managed `GameServer`s with a live pod, for the scheduled backup cycle.
/// Shut-down instances (Service only, no pod) are absent — there is no supervisor
/// to stream from — and are recovered via archive, not backed up.
///
/// # Errors
///
/// Returns an error if the gameservers can't be listed from the Kubernetes API.
pub(crate) async fn list_backup_targets(
    client: &Client,
    namespace: &str,
) -> Result<Vec<BackupTarget>> {
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
        targets.push(BackupTarget {
            instance,
            game: label_value(labels, GAME_KEY).unwrap_or_default().to_owned(),
            guild: label_value(labels, GUILD_KEY)
                .unwrap_or_default()
                .to_owned(),
            running,
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
            Some(STOPPED_STATE),
            node_port,
            domain,
        ));
    }
}
