use std::collections::HashMap;

use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::Service;
use kube::api::ListParams;
use kube::{Api, Client};
use tracing::warn;

use super::types::{GameServer, ServerSummary, summarize};

const GAMESERVER_SELECTOR_KEY: &str = "agones.dev/gameserver";

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
    for gameserver in &gs_list.items {
        let name = gameserver.metadata.name.as_deref().unwrap_or("<unnamed>");
        let state = gameserver
            .status
            .as_ref()
            .and_then(|status| status.state.as_deref());
        let node_port = node_port_by_server.get(name).copied();
        if node_port.is_none() {
            warn!(
                gameserver = name,
                "no NodePort service matched gameserver; address omitted"
            );
        }
        summaries.push(summarize(name, state, node_port, domain));
    }

    Ok(summaries)
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
