use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::Service;
use kube::api::ListParams;
use kube::{Api, Client};
use tracing::warn;

use grizzly_control_api::{PROCESS_LABEL_KEY, PROCESS_LABEL_STOPPED};

use super::labels::{GAME_KEY, GAMESERVER_SELECTOR_KEY, is_managed};
use super::types::{GameServer, ServerSummary, summarize};

/// State label shown for a managed instance whose Service (and leased port)
/// still exist but whose `GameServer` has been torn down by `/shutdown`.
const STOPPED_STATE: &str = "Stopped";

/// State shown for a server whose pod is up but whose game process the
/// supervisor has paused (`/stop`). Distinct from `Stopped` (= shut down).
const PAUSED_STATE: &str = "Paused";

/// List every Agones `GameServer` in `namespace`, joining each to its
/// `NodePort` Service to resolve a connection address under `domain`.
///
/// # Errors
///
/// Returns an error if the gameservers or services cannot be listed from the
/// Kubernetes API.
pub(crate) async fn list_active_servers(
    client: Client,
    namespace: &str,
    domain: &str,
) -> Result<Vec<ServerSummary>> {
    let gameservers: Api<GameServer> = Api::namespaced(client.clone(), namespace);
    let services: Api<Service> = Api::namespaced(client, namespace);

    let gs_list = gameservers
        .list(&ListParams::default())
        .await
        .with_context(|| format!("failed to list gameservers in namespace {namespace}"))?;
    let svc_list = services
        .list(&ListParams::default())
        .await
        .with_context(|| format!("failed to list services in namespace {namespace}"))?;

    let node_port_by_server = node_ports_by_gameserver(&svc_list.items);

    let mut summaries = Vec::with_capacity(gs_list.items.len());
    let mut live: HashSet<&str> = HashSet::new();
    for gameserver in &gs_list.items {
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
        let Some(spec) = service.spec.as_ref() else {
            continue;
        };
        let Some(target) = spec
            .selector
            .as_ref()
            .and_then(|selector| selector.get(GAMESERVER_SELECTOR_KEY))
        else {
            continue;
        };
        if live.contains(target.as_str()) {
            continue;
        }
        let node_port = spec
            .ports
            .as_ref()
            .and_then(|ports| ports.iter().find_map(|port| port.node_port));
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

/// Read a single label value off an object's label map, if present.
fn label_value<'a>(labels: Option<&'a BTreeMap<String, String>>, key: &str) -> Option<&'a str> {
    labels.and_then(|map| map.get(key)).map(String::as_str)
}

/// Map each `NodePort` Service's targeted gameserver (via its
/// `agones.dev/gameserver` selector) to the Service's first `NodePort`.
fn node_ports_by_gameserver(services: &[Service]) -> HashMap<String, i32> {
    let mut ports = HashMap::new();
    for service in services {
        let Some(spec) = service.spec.as_ref() else {
            continue;
        };
        let Some(selector) = spec.selector.as_ref() else {
            continue;
        };
        let Some(gameserver) = selector.get(GAMESERVER_SELECTOR_KEY) else {
            continue;
        };
        let Some(service_ports) = spec.ports.as_ref() else {
            continue;
        };
        let Some(node_port) = service_ports.iter().find_map(|port| port.node_port) else {
            continue;
        };
        ports.insert(gameserver.clone(), node_port);
    }
    ports
}
