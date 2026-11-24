use std::collections::HashSet;

use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::api::ListParams;
use kube::{Api, Client};
use tracing::warn;

use grizzly_control_api::{PROCESS_LABEL_KEY, PROCESS_LABEL_STOPPED};

use super::labels::{
    GAME_KEY, GAMESERVER_SELECTOR_KEY, GUILD_KEY, is_managed, label_value,
    service_gameserver_target,
};
use super::ports::{friend_facing_node_port, node_ports_by_gameserver};
use super::provision::{DeletionState, deletion_state};
use super::scope::ServerScope;
use super::types::{GameServer, ServerState, ServerSummary, summarize};
use crate::domain::{GameId, GuildId, InstanceName};
use crate::events::declared_resources;
use crate::limits::GuildUsage;

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
        if !is_managed(gameserver.metadata.labels.as_ref()) {
            continue;
        }
        let instance = gameserver.metadata.name.as_deref().unwrap_or("<unnamed>");
        live.insert(instance);
        let paused = label_value(gameserver.metadata.labels.as_ref(), PROCESS_LABEL_KEY)
            == Some(PROCESS_LABEL_STOPPED);
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

const UNHEALTHY_STATE: &str = "Unhealthy";

pub(crate) struct LiveInstance {
    pub(crate) instance: InstanceName,
    pub(crate) game: GameId,
    pub(crate) guild: GuildId,
    pub(crate) running: bool,
    pub(crate) unhealthy: bool,
}

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

pub(crate) async fn guild_usage(
    client: &Client,
    namespace: &str,
    guild: &str,
    exclude_instance: Option<&str>,
) -> Result<GuildUsage> {
    let scope = ServerScope::Guild(guild.to_owned());
    let mut params = ListParams::default();
    if let Some(selector) = scope.label_selector() {
        params = params.labels(&selector);
    }

    let services: Api<Service> = Api::namespaced(client.clone(), namespace);
    let svc_list = services
        .list(&params)
        .await
        .with_context(|| format!("failed to list services in namespace {namespace}"))?;
    let mut instances: HashSet<&str> = HashSet::new();
    for service in &svc_list.items {
        if !is_managed(service.metadata.labels.as_ref()) {
            continue;
        }
        if let Some(target) = service_gameserver_target(service) {
            instances.insert(target);
        }
    }

    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let pod_list = pods
        .list(&ListParams::default().labels(GAMESERVER_SELECTOR_KEY))
        .await
        .with_context(|| format!("failed to list game pods in namespace {namespace}"))?;
    let mut live_cpu_limit_millicores: i64 = 0;
    let mut live_mem_limit_bytes: i64 = 0;
    for pod in &pod_list.items {
        let Some(instance) = label_value(pod.metadata.labels.as_ref(), GAMESERVER_SELECTOR_KEY)
        else {
            continue;
        };
        if !instances.contains(instance) || exclude_instance == Some(instance) {
            continue;
        }
        let declared = declared_resources(pod);
        live_cpu_limit_millicores =
            live_cpu_limit_millicores.saturating_add(declared.cpu_limit_millicores.unwrap_or(0));
        live_mem_limit_bytes =
            live_mem_limit_bytes.saturating_add(declared.mem_limit_bytes.unwrap_or(0));
    }

    Ok(GuildUsage {
        server_count: instances.len(),
        live_cpu_limit_millicores,
        live_mem_limit_bytes,
    })
}

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
