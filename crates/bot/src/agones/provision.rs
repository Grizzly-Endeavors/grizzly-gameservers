use std::collections::BTreeSet;
use std::ops::RangeInclusive;
use std::time::Duration;

use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::{PersistentVolumeClaim, Service};
use kube::api::{Api, DeleteParams, ListParams, PostParams};
use kube::core::{ApiResource, DynamicObject, GroupVersionKind};
use kube::{Client, Error as KubeError};
use tokio::sync::Mutex;
use tracing::{debug, error, warn};

use super::catalog::{GameCatalog, GameCatalogEntry};
use super::instance::{InstanceIdentity, render_gameserver, render_pvc, render_service};
use super::labels::{CHANNEL_KEY, GAME_KEY, is_managed};
use super::naming::{pvc_name, select_free_port};
use super::scope::ServerScope;
use super::types::{GameServer, server_address};

/// Public `NodePort` band the edge VPS forwards 1:1 over the tunnel. Per-instance
/// Services lease a port from here; the range bounds how many servers can run.
const PORT_RANGE: RangeInclusive<i32> = 7000..=7010;

/// How long `/create` and `/start` wait for a server to report Ready before
/// telling the friend it is "still starting". Generous because first-boot world
/// generation plus the readiness sidecar's SDK call can take minutes.
const READY_TIMEOUT: Duration = Duration::from_mins(5);
const POLL_INTERVAL: Duration = Duration::from_secs(3);

/// Outcome of a `/create`. Expected friend-visible results (name clash, no free
/// ports) are values, not errors; `Err` is reserved for cluster/internal faults.
pub(crate) enum CreateOutcome {
    Created { address: String, ready: bool },
    AlreadyExists,
    PortsExhausted,
}

pub(crate) enum ShutdownOutcome {
    Down,
    NotFound,
    NotManaged,
}

pub(crate) enum StartOutcome {
    Started { address: String, ready: bool },
    AlreadyRunning,
    NotFound,
    NotManaged,
    UnknownGame(String),
}

pub(crate) enum DestroyOutcome {
    Destroyed,
    NotFound,
    NotManaged,
}

/// Result of the provisioning phase of `/create`: everything up to — but not
/// including — waiting for the server to report Ready. Split from readiness so
/// the caller can surface the connection address the moment it is leased and
/// then poll with [`wait_for_instance_ready`], giving the friend live progress
/// instead of one silent multi-minute wait.
pub(crate) enum ProvisionOutcome {
    Provisioned { address: String },
    AlreadyExists,
    PortsExhausted,
}

/// Provision a new per-world instance: lease a port, create its Service, PVC and
/// `GameServer`, and return its address. Does **not** wait for readiness — call
/// [`wait_for_instance_ready`] for that. The `lock` serializes the
/// port-lease→Service-create critical section so concurrent creates can't claim
/// the same `NodePort`. `channel` is the owning Discord channel id, stamped onto
/// the trio so scoped listing and mutation can confine to it later.
///
/// # Errors
///
/// Returns an error if the cluster cannot be reached or an object create fails
/// for a reason other than a recoverable port clash.
pub(crate) async fn provision_instance(
    client: &Client,
    namespace: &str,
    domain: &str,
    lock: &Mutex<()>,
    entry: &GameCatalogEntry,
    instance: &str,
    channel: &str,
) -> Result<ProvisionOutcome> {
    let _guard = lock.lock().await;
    let provisioned =
        provision_under_lock(client, namespace, domain, entry, instance, channel, false).await?;
    Ok(provisioned.into_outcome())
}

/// Provision an instance held down at boot (`SUPERVISOR_START_PAUSED`) so the
/// caller can seed `/data` from an archive before the game first launches. Same
/// contract as [`provision_instance`] otherwise; used only by recover-from-archive.
///
/// # Errors
///
/// Returns an error if the cluster cannot be reached or an object create fails
/// for a reason other than a recoverable port clash.
pub(crate) async fn provision_paused_instance(
    client: &Client,
    namespace: &str,
    domain: &str,
    lock: &Mutex<()>,
    entry: &GameCatalogEntry,
    instance: &str,
    channel: &str,
) -> Result<ProvisionOutcome> {
    let _guard = lock.lock().await;
    let provisioned =
        provision_under_lock(client, namespace, domain, entry, instance, channel, true).await?;
    Ok(provisioned.into_outcome())
}

enum Provisioned {
    Created(String),
    AlreadyExists,
    PortsExhausted,
}

impl Provisioned {
    fn into_outcome(self) -> ProvisionOutcome {
        match self {
            Self::Created(address) => ProvisionOutcome::Provisioned { address },
            Self::AlreadyExists => ProvisionOutcome::AlreadyExists,
            Self::PortsExhausted => ProvisionOutcome::PortsExhausted,
        }
    }
}

async fn provision_under_lock(
    client: &Client,
    namespace: &str,
    domain: &str,
    entry: &GameCatalogEntry,
    instance: &str,
    channel: &str,
    start_paused: bool,
) -> Result<Provisioned> {
    if instance_exists(client, namespace, instance).await? {
        return Ok(Provisioned::AlreadyExists);
    }

    let used = used_ports(client, namespace).await?;
    let mut excluded = BTreeSet::new();
    let services: Api<Service> = Api::namespaced(client.clone(), namespace);

    loop {
        let Some(port) = select_free_port(&used, &excluded, PORT_RANGE) else {
            return Ok(Provisioned::PortsExhausted);
        };
        let identity = InstanceIdentity {
            name: instance.to_owned(),
            game: entry.id.clone(),
            namespace: namespace.to_owned(),
            node_port: port,
            channel: channel.to_owned(),
            start_paused,
        };
        let service = render_service(entry, &identity)?;
        match services.create(&PostParams::default(), &service).await {
            Ok(_) => {}
            Err(err) if is_port_conflict(&err) => {
                warn!(
                    port,
                    instance, "nodeport already taken, retrying with next free port"
                );
                excluded.insert(port);
                continue;
            }
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to create service for {instance}"));
            }
        }

        if let Err(err) = create_storage_and_server(client, namespace, entry, &identity).await {
            error!(error = ?err, instance, "create failed after service; rolling back");
            best_effort_remove(client, namespace, instance).await;
            return Err(err);
        }
        return Ok(Provisioned::Created(server_address(instance, domain, port)));
    }
}

async fn create_storage_and_server(
    client: &Client,
    namespace: &str,
    entry: &GameCatalogEntry,
    identity: &InstanceIdentity,
) -> Result<()> {
    let pvc = render_pvc(entry, identity)?;
    let pvcs: Api<PersistentVolumeClaim> = Api::namespaced(client.clone(), namespace);
    pvcs.create(&PostParams::default(), &pvc)
        .await
        .with_context(|| format!("failed to create pvc for {}", identity.name))?;

    let gameserver = render_gameserver(entry, identity)?;
    gameserver_api(client, namespace)
        .create(&PostParams::default(), &gameserver)
        .await
        .with_context(|| format!("failed to create gameserver for {}", identity.name))?;
    Ok(())
}

/// Shut down a running instance: delete only its `GameServer`, leaving the Service
/// (and its leased port) and the PVC in place so `/start` can recreate it. This
/// is the heavier teardown — the pod is gone and a cold `/start` reschedules it.
/// The lighter `/stop` (pause the process, keep the pod) goes through the
/// supervisor, not here.
///
/// # Errors
///
/// Returns an error if the cluster cannot be reached.
pub(crate) async fn shutdown_instance(
    client: &Client,
    namespace: &str,
    instance: &str,
) -> Result<ShutdownOutcome> {
    let services: Api<Service> = Api::namespaced(client.clone(), namespace);
    let Some(service) = services
        .get_opt(instance)
        .await
        .with_context(|| format!("failed to read service {instance}"))?
    else {
        return Ok(ShutdownOutcome::NotFound);
    };
    if !is_managed(service.metadata.labels.as_ref()) {
        return Ok(ShutdownOutcome::NotManaged);
    }
    delete_if_present(&gameserver_api(client, namespace), instance).await?;
    Ok(ShutdownOutcome::Down)
}

/// Result of the start-up phase of `/start`: the `GameServer` has been recreated
/// (or the instance can't be started), but readiness is not yet awaited. As with
/// [`ProvisionOutcome`], the caller surfaces the address right away and then
/// polls [`wait_for_instance_ready`].
pub(crate) enum StartBegin {
    Starting { address: String },
    AlreadyRunning,
    NotFound,
    NotManaged,
    UnknownGame(String),
}

/// Recreate a previously stopped instance's `GameServer`, bound to the existing
/// PVC and reusing the retained Service and its port. Does **not** wait for
/// readiness — call [`wait_for_instance_ready`] for that.
///
/// # Errors
///
/// Returns an error if the cluster cannot be reached, the retained Service is
/// malformed, or the `GameServer` create fails for a reason other than the server
/// already running.
pub(crate) async fn begin_start(
    client: &Client,
    namespace: &str,
    domain: &str,
    catalog: &GameCatalog,
    instance: &str,
) -> Result<StartBegin> {
    let services: Api<Service> = Api::namespaced(client.clone(), namespace);
    let Some(service) = services
        .get_opt(instance)
        .await
        .with_context(|| format!("failed to read service {instance}"))?
    else {
        return Ok(StartBegin::NotFound);
    };
    if !is_managed(service.metadata.labels.as_ref()) {
        return Ok(StartBegin::NotManaged);
    }

    let game = service
        .metadata
        .labels
        .as_ref()
        .and_then(|labels| labels.get(GAME_KEY))
        .cloned()
        .with_context(|| format!("managed service {instance} is missing its game label"))?;
    let Some(entry) = catalog.get(&game) else {
        return Ok(StartBegin::UnknownGame(game));
    };
    let node_port = service_node_port(&service)
        .with_context(|| format!("managed service {instance} has no nodeport"))?;
    // Carry the owning channel from the surviving Service so the recreated
    // GameServer keeps its scope; empty for a pre-scoping instance (label
    // absent), which leaves the channel label off rather than stamping "".
    let channel = service
        .metadata
        .labels
        .as_ref()
        .and_then(|labels| labels.get(CHANNEL_KEY))
        .cloned()
        .unwrap_or_default();

    let identity = InstanceIdentity {
        name: instance.to_owned(),
        game,
        namespace: namespace.to_owned(),
        node_port,
        channel,
        // A cold `/start` resumes a normal server; only recover-from-archive pauses.
        start_paused: false,
    };
    let gameserver = render_gameserver(entry, &identity)?;
    match gameserver_api(client, namespace)
        .create(&PostParams::default(), &gameserver)
        .await
    {
        Ok(_) => {}
        Err(err) if is_already_exists(&err) => return Ok(StartBegin::AlreadyRunning),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to start gameserver {instance}"));
        }
    }
    Ok(StartBegin::Starting {
        address: server_address(instance, domain, node_port),
    })
}

/// Tear an instance down completely: delete its `GameServer`, Service and PVC.
/// This destroys the world.
///
/// # Errors
///
/// Returns an error if the cluster cannot be reached or a delete fails for a
/// reason other than the object already being gone.
pub(crate) async fn destroy_instance(
    client: &Client,
    namespace: &str,
    instance: &str,
) -> Result<DestroyOutcome> {
    let services: Api<Service> = Api::namespaced(client.clone(), namespace);
    let Some(service) = services
        .get_opt(instance)
        .await
        .with_context(|| format!("failed to read service {instance}"))?
    else {
        return Ok(DestroyOutcome::NotFound);
    };
    if !is_managed(service.metadata.labels.as_ref()) {
        return Ok(DestroyOutcome::NotManaged);
    }
    delete_if_present(&gameserver_api(client, namespace), instance).await?;
    delete_if_present(&services, instance).await?;
    let pvcs: Api<PersistentVolumeClaim> = Api::namespaced(client.clone(), namespace);
    delete_if_present(&pvcs, &pvc_name(instance)).await?;
    Ok(DestroyOutcome::Destroyed)
}

/// Names of the shim-managed instances (running or stopped) visible under
/// `scope`, for autocomplete — so a friend only completes their own channel's
/// servers.
///
/// # Errors
///
/// Returns an error if services cannot be listed.
pub(crate) async fn list_instance_names(
    client: &Client,
    namespace: &str,
    scope: &ServerScope,
) -> Result<Vec<String>> {
    let services: Api<Service> = Api::namespaced(client.clone(), namespace);
    let mut params = ListParams::default();
    if let Some(selector) = scope.label_selector() {
        params = params.labels(&selector);
    }
    let list = services
        .list(&params)
        .await
        .with_context(|| format!("failed to list services in namespace {namespace}"))?;
    let mut names: Vec<String> = list
        .items
        .iter()
        .filter(|service| is_managed(service.metadata.labels.as_ref()))
        .filter_map(|service| service.metadata.name.clone())
        .collect();
    names.sort();
    Ok(names)
}

fn gameserver_api(client: &Client, namespace: &str) -> Api<DynamicObject> {
    let gvk = GroupVersionKind::gvk("agones.dev", "v1", "GameServer");
    let resource = ApiResource::from_gvk(&gvk);
    Api::namespaced_with(client.clone(), namespace, &resource)
}

async fn instance_exists(client: &Client, namespace: &str, instance: &str) -> Result<bool> {
    let services: Api<Service> = Api::namespaced(client.clone(), namespace);
    if services
        .get_opt(instance)
        .await
        .with_context(|| format!("failed to check for existing service {instance}"))?
        .is_some()
    {
        return Ok(true);
    }
    let exists = gameserver_api(client, namespace)
        .get_opt(instance)
        .await
        .with_context(|| format!("failed to check for existing gameserver {instance}"))?
        .is_some();
    Ok(exists)
}

async fn used_ports(client: &Client, namespace: &str) -> Result<BTreeSet<i32>> {
    let services: Api<Service> = Api::namespaced(client.clone(), namespace);
    let list = services
        .list(&ListParams::default())
        .await
        .with_context(|| format!("failed to list services in namespace {namespace}"))?;

    let mut ports = BTreeSet::new();
    for service in &list.items {
        if let Some(port) = service_node_port(service)
            && PORT_RANGE.contains(&port)
        {
            ports.insert(port);
        }
    }
    Ok(ports)
}

fn service_node_port(service: &Service) -> Option<i32> {
    service
        .spec
        .as_ref()?
        .ports
        .as_ref()?
        .iter()
        .find_map(|port| port.node_port)
}

/// Poll a `GameServer` until it reports Ready/Allocated, returning `false` if it
/// hasn't come up within [`READY_TIMEOUT`] (e.g. first-boot world generation is
/// still running). Called after [`provision_instance`] / [`begin_start`].
///
/// # Errors
///
/// Returns an error if the gameserver cannot be polled from the Kubernetes API.
pub(crate) async fn wait_for_instance_ready(
    client: &Client,
    namespace: &str,
    instance: &str,
) -> Result<bool> {
    let gameservers: Api<GameServer> = Api::namespaced(client.clone(), namespace);
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    let mut ticker = tokio::time::interval(POLL_INTERVAL);

    loop {
        ticker.tick().await;
        match gameservers.get_opt(instance).await {
            Ok(Some(gameserver)) => {
                if is_ready(&gameserver) {
                    return Ok(true);
                }
            }
            Ok(None) => debug!(instance, "gameserver not yet visible to the api"),
            Err(err) => {
                return Err(err).with_context(|| format!("failed to poll readiness of {instance}"));
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
    }
}

fn is_ready(gameserver: &GameServer) -> bool {
    matches!(
        gameserver
            .status
            .as_ref()
            .and_then(|status| status.state.as_deref()),
        Some("Ready" | "Allocated")
    )
}

async fn best_effort_remove(client: &Client, namespace: &str, instance: &str) {
    let services: Api<Service> = Api::namespaced(client.clone(), namespace);
    let pvcs: Api<PersistentVolumeClaim> = Api::namespaced(client.clone(), namespace);
    for outcome in [
        delete_if_present(&gameserver_api(client, namespace), instance).await,
        delete_if_present(&services, instance).await,
        delete_if_present(&pvcs, &pvc_name(instance)).await,
    ] {
        if let Err(err) = outcome {
            warn!(error = ?err, instance, "rollback delete failed; manual cleanup may be needed");
        }
    }
}

async fn delete_if_present<K>(api: &Api<K>, name: &str) -> Result<()>
where
    K: Clone + serde::de::DeserializeOwned + std::fmt::Debug,
{
    match api.delete(name, &DeleteParams::default()).await {
        Ok(_) => Ok(()),
        Err(err) if is_not_found(&err) => {
            debug!(name, "object already absent");
            Ok(())
        }
        Err(err) => Err(err).with_context(|| format!("failed to delete {name}")),
    }
}

fn api_status_code(err: &KubeError) -> Option<u16> {
    if let KubeError::Api(response) = err {
        Some(response.code)
    } else {
        None
    }
}

fn is_not_found(err: &KubeError) -> bool {
    api_status_code(err) == Some(404)
}

fn is_already_exists(err: &KubeError) -> bool {
    api_status_code(err) == Some(409)
}

fn is_port_conflict(err: &KubeError) -> bool {
    api_status_code(err) == Some(422)
}
