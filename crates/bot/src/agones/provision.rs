use std::collections::BTreeSet;
use std::ops::RangeInclusive;
use std::time::Duration;

use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::{PersistentVolumeClaim, Service};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{Api, DeleteParams, ListParams, PostParams};
use kube::core::{ApiResource, DynamicObject, GroupVersionKind};
use kube::{Client, Error as KubeError};
use tokio::sync::Mutex;
use tracing::{debug, error, warn};

use super::capacity::{check_namespace_capacity, create_admission, start_admission};
use super::catalog::{GameCatalog, GameCatalogEntry};
use super::client::guild_usage;
use super::instance::{
    InstanceIdentity, catalog_sizing, render_gameserver, render_pvc, render_service,
};
use super::labels::{GAME_KEY, GUILD_KEY, all_node_ports, is_managed, label_value};
use super::naming::{pvc_name, select_free_ports};
use super::ports::{assign, assignment_from_service, port_plan_from_service, ports_needed};
use super::scope::ServerScope;
use super::types::{GameServer, ServerState, server_address};
use crate::capacity::CapacityRefusal;
use crate::domain::{GameId, GuildId, InstanceName};
use crate::limits::{GuildCaps, LimitRefusal, check_create};
use crate::sizing::Sizing;

const PORT_RANGE: RangeInclusive<i32> = 7000..=7099;

const READY_TIMEOUT: Duration = Duration::from_mins(5);
const POLL_INTERVAL: Duration = Duration::from_secs(3);

/// How long a delete waits for the object to actually leave the API before
/// calling it stranded. A `GameServer` carries Agones' `agones.dev/controller`
/// finalizer, which Agones only releases once the pod is truly gone — so this
/// has to clear the game pods' `terminationGracePeriodSeconds` (120s, the
/// supervisor's world-save window) with room to spare. Past that, the pod's
/// kubelet isn't answering and the object is pinned until its node returns.
const DELETE_TIMEOUT: Duration = Duration::from_secs(150);

pub(crate) enum CreateOutcome {
    Created { address: String, ready: bool },
    AlreadyExists,
    NameUnavailable,
    PortsExhausted,
    GuildLimitReached(LimitRefusal),
    ClusterFull(CapacityRefusal),
}

pub(crate) enum ShutdownOutcome {
    Down,
    Stranded,
    NotFound,
    NotManaged,
}

pub(crate) enum StartOutcome {
    Started { address: String, ready: bool },
    AlreadyRunning,
    StillStopping,
    NotFound,
    NotManaged,
    UnknownGame(String),
    ClusterFull(CapacityRefusal),
}

pub(crate) enum DestroyOutcome {
    Destroyed,
    Blocked,
    NotFound,
    NotManaged,
}

pub(crate) enum ProvisionOutcome {
    Provisioned { address: String },
    AlreadyExists,
    NameUnavailable,
    PortsExhausted,
    GuildLimitReached(LimitRefusal),
    ClusterFull(CapacityRefusal),
}

pub(crate) struct ProvisionSpec<'a> {
    pub(crate) entry: &'a GameCatalogEntry,
    pub(crate) instance: &'a str,
    pub(crate) guild: &'a str,
    pub(crate) caps: Option<&'a GuildCaps>,
}

pub(crate) async fn provision_instance(
    client: &Client,
    namespace: &str,
    domain: &str,
    lock: &Mutex<()>,
    spec: ProvisionSpec<'_>,
) -> Result<ProvisionOutcome> {
    let _guard = lock.lock().await;
    let provisioned = provision_under_lock(client, namespace, domain, &spec, false).await?;
    Ok(provisioned.into_outcome())
}

pub(crate) async fn provision_paused_instance(
    client: &Client,
    namespace: &str,
    domain: &str,
    lock: &Mutex<()>,
    spec: ProvisionSpec<'_>,
) -> Result<ProvisionOutcome> {
    let _guard = lock.lock().await;
    let provisioned = provision_under_lock(client, namespace, domain, &spec, true).await?;
    Ok(provisioned.into_outcome())
}

enum Provisioned {
    Created(String),
    AlreadyExists,
    NameUnavailable,
    PortsExhausted,
    GuildLimitReached(LimitRefusal),
    ClusterFull(CapacityRefusal),
}

impl Provisioned {
    fn into_outcome(self) -> ProvisionOutcome {
        match self {
            Self::Created(address) => ProvisionOutcome::Provisioned { address },
            Self::AlreadyExists => ProvisionOutcome::AlreadyExists,
            Self::NameUnavailable => ProvisionOutcome::NameUnavailable,
            Self::PortsExhausted => ProvisionOutcome::PortsExhausted,
            Self::GuildLimitReached(refusal) => ProvisionOutcome::GuildLimitReached(refusal),
            Self::ClusterFull(refusal) => ProvisionOutcome::ClusterFull(refusal),
        }
    }
}

async fn provision_under_lock(
    client: &Client,
    namespace: &str,
    domain: &str,
    spec: &ProvisionSpec<'_>,
    start_paused: bool,
) -> Result<Provisioned> {
    let ProvisionSpec {
        entry,
        instance,
        guild,
        caps,
    } = *spec;
    match name_holder(client, namespace, instance, guild).await? {
        NameHolder::Free => {}
        NameHolder::Caller => return Ok(Provisioned::AlreadyExists),
        NameHolder::Other => return Ok(Provisioned::NameUnavailable),
    }

    if let Some(caps) = caps {
        let usage = guild_usage(client, namespace, guild, None).await?;
        if let Err(refusal) = check_create(&usage, catalog_sizing(entry), caps) {
            return Ok(Provisioned::GuildLimitReached(refusal));
        }
    }

    // The namespace ceiling, priced before anything is leased. Kubernetes only
    // enforces the quota when Agones creates the pod — minutes later, long after
    // this call returned an address — so without this the friend gets a port, a
    // PVC and a five-minute wait for a pod that was never going to be admitted.
    // Checked under the same lock as the guild caps, for the same reason.
    if let Err(refusal) =
        check_namespace_capacity(client, namespace, create_admission(entry, None)).await?
    {
        return Ok(Provisioned::ClusterFull(refusal));
    }

    let used = used_ports(client, namespace).await?;
    let mut excluded = BTreeSet::new();
    let services: Api<Service> = Api::namespaced(client.clone(), namespace);

    let template: Service = serde_saphyr::from_str(&entry.service_yaml)
        .with_context(|| format!("failed to parse service template for game {}", entry.id))?;
    let plan = port_plan_from_service(&template)
        .with_context(|| format!("invalid port plan for game {}", entry.id))?;
    let needed = ports_needed(&plan);

    loop {
        let Some(leased) = select_free_ports(needed, &used, &excluded, PORT_RANGE) else {
            return Ok(Provisioned::PortsExhausted);
        };
        let identity = InstanceIdentity {
            name: InstanceName::new(instance),
            game: GameId::new(entry.id.as_str()),
            namespace: namespace.to_owned(),
            ports: assign(plan.clone(), &leased)?,
            guild: GuildId::new(guild),
            start_paused,
            sizing: None,
        };
        let service = render_service(entry, &identity)?;
        match services.create(&PostParams::default(), &service).await {
            Ok(_) => {}
            Err(err) if is_port_conflict(&err) => {
                debug!(
                    ?leased,
                    instance, "nodeport already taken, retrying with next free ports"
                );
                excluded.extend(leased.iter().copied());
                continue;
            }
            Err(err) if is_already_exists(&err) => return Ok(Provisioned::AlreadyExists),
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to create service for {instance}"));
            }
        }

        let address = server_address(instance, domain, identity.ports.friend_facing_port());
        match create_storage_and_server(client, namespace, entry, &identity).await {
            Ok(()) => {}
            Err(err) if is_quota_exceeded(&err) => {
                warn!(
                    instance,
                    "admission refused the create: namespace quota is exhausted"
                );
                best_effort_remove(client, namespace, instance).await;
                return Ok(Provisioned::ClusterFull(
                    quota_refusal(client, namespace, entry).await,
                ));
            }
            Err(err) => {
                error!(error = ?err, instance, "create failed after service; rolling back");
                best_effort_remove(client, namespace, instance).await;
                return Err(err);
            }
        }
        return Ok(Provisioned::Created(address));
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
/// Waits for the `GameServer` to actually leave the API rather than trusting the
/// accepted delete: Agones' finalizer keeps the object addressable until the pod
/// is reaped, and a caller that assumes otherwise reports a shutdown that hasn't
/// happened and routes the next `/start` down the warm path into a dead pod.
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
    match delete_and_confirm(&gameserver_api(client, namespace), instance).await? {
        DeleteWait::Deleted => Ok(ShutdownOutcome::Down),
        DeleteWait::Unconfirmed => Ok(ShutdownOutcome::Stranded),
    }
}

pub(crate) enum StartBegin {
    Starting { address: String },
    AlreadyRunning,
    NotFound,
    NotManaged,
    UnknownGame(String),
    ClusterFull(CapacityRefusal),
}

pub(crate) async fn begin_start(
    client: &Client,
    namespace: &str,
    domain: &str,
    catalog: &GameCatalog,
    instance: &str,
    sizing: Option<Sizing>,
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

    let game = label_value(service.metadata.labels.as_ref(), GAME_KEY)
        .map(str::to_owned)
        .with_context(|| format!("managed service {instance} is missing its game label"))?;
    let Some(entry) = catalog.get(&game) else {
        return Ok(StartBegin::UnknownGame(game));
    };
    let ports = assignment_from_service(&service)
        .with_context(|| format!("managed service {instance} has an unusable port assignment"))?;
    let guild = label_value(service.metadata.labels.as_ref(), GUILD_KEY)
        .unwrap_or_default()
        .to_owned();

    if let Err(refusal) =
        check_namespace_capacity(client, namespace, start_admission(entry, sizing)).await?
    {
        return Ok(StartBegin::ClusterFull(refusal));
    }

    let identity = InstanceIdentity {
        name: InstanceName::new(instance),
        game: GameId::new(game),
        namespace: namespace.to_owned(),
        ports,
        guild: GuildId::new(guild),
        start_paused: false,
        sizing,
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
        address: server_address(instance, domain, identity.ports.friend_facing_port()),
    })
}

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
    if matches!(
        delete_and_confirm(&gameserver_api(client, namespace), instance).await?,
        DeleteWait::Unconfirmed
    ) {
        warn!(
            instance,
            "gameserver did not confirm gone; leaving service and pvc intact"
        );
        return Ok(DestroyOutcome::Blocked);
    }
    delete_if_present(&services, instance).await?;
    let pvcs: Api<PersistentVolumeClaim> = Api::namespaced(client.clone(), namespace);
    delete_if_present(&pvcs, &pvc_name(instance)).await?;
    Ok(DestroyOutcome::Destroyed)
}

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

pub(super) fn gameserver_api(client: &Client, namespace: &str) -> Api<DynamicObject> {
    let gvk = GroupVersionKind::gvk("agones.dev", "v1", "GameServer");
    let resource = ApiResource::from_gvk(&gvk);
    Api::namespaced_with(client.clone(), namespace, &resource)
}

enum NameHolder {
    Free,
    Caller,
    Other,
}

async fn name_holder(
    client: &Client,
    namespace: &str,
    instance: &str,
    guild: &str,
) -> Result<NameHolder> {
    let services: Api<Service> = Api::namespaced(client.clone(), namespace);
    if let Some(service) = services
        .get_opt(instance)
        .await
        .with_context(|| format!("failed to check for existing service {instance}"))?
    {
        let holder = label_value(service.metadata.labels.as_ref(), GUILD_KEY);
        return Ok(classify_holder(holder, guild));
    }
    let gameserver_exists = gameserver_api(client, namespace)
        .get_opt(instance)
        .await
        .with_context(|| format!("failed to check for existing gameserver {instance}"))?
        .is_some();
    Ok(if gameserver_exists {
        NameHolder::Other
    } else {
        NameHolder::Free
    })
}

fn classify_holder(holder: Option<&str>, guild: &str) -> NameHolder {
    match holder {
        Some(holder) if !guild.is_empty() && holder == guild => NameHolder::Caller,
        Some(_) | None => NameHolder::Other,
    }
}

async fn used_ports(client: &Client, namespace: &str) -> Result<BTreeSet<i32>> {
    let services: Api<Service> = Api::namespaced(client.clone(), namespace);
    let list = services
        .list(&ListParams::default())
        .await
        .with_context(|| format!("failed to list services in namespace {namespace}"))?;

    let mut ports = BTreeSet::new();
    for service in &list.items {
        for port in all_node_ports(service) {
            if PORT_RANGE.contains(&port) {
                ports.insert(port);
            }
        }
    }
    Ok(ports)
}

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
    gameserver
        .status
        .as_ref()
        .and_then(|status| status.state.as_deref())
        .map(ServerState::from_agones)
        .is_some_and(|state| state.is_online())
}

/// Roll back a half-built instance. Deletes in reverse-create order — the
/// `GameServer` first so Agones releases the pod, then the PVC it mounted, then
/// the Service — because a PVC delete issued while its pod is alive only takes
/// effect later, once `kubernetes.io/pvc-protection` clears.
async fn best_effort_remove(client: &Client, namespace: &str, instance: &str) {
    let services: Api<Service> = Api::namespaced(client.clone(), namespace);
    let pvcs: Api<PersistentVolumeClaim> = Api::namespaced(client.clone(), namespace);
    let gameserver = delete_and_confirm(&gameserver_api(client, namespace), instance)
        .await
        .map(|_| ());
    for outcome in [
        gameserver,
        delete_if_present(&pvcs, &pvc_name(instance)).await,
        delete_if_present(&services, instance).await,
    ] {
        if let Err(err) = outcome {
            warn!(error = ?err, instance, "rollback delete failed; manual cleanup may be needed");
        }
    }
}

/// Where an object sits in its delete lifecycle, read from its metadata alone.
/// Kubernetes answers a delete as soon as it is *accepted*, so "the API returned
/// success" and "the object is gone" are different facts whenever a finalizer is
/// involved — this is the distinction the callers need to keep straight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeletionState {
    Live,
    Terminating,
}

pub(crate) fn deletion_state(meta: &ObjectMeta) -> DeletionState {
    if meta.deletion_timestamp.is_some() {
        DeletionState::Terminating
    } else {
        DeletionState::Live
    }
}

pub(crate) enum DeleteWait {
    Deleted,
    Unconfirmed,
}

pub(super) async fn delete_and_confirm<K>(api: &Api<K>, name: &str) -> Result<DeleteWait>
where
    K: Clone + serde::de::DeserializeOwned + std::fmt::Debug + kube::Resource,
{
    delete_if_present(api, name).await?;

    let deadline = tokio::time::Instant::now() + DELETE_TIMEOUT;
    let mut ticker = tokio::time::interval(POLL_INTERVAL);
    loop {
        ticker.tick().await;
        let Some(object) = api
            .get_opt(name)
            .await
            .with_context(|| format!("failed to confirm deletion of {name}"))?
        else {
            return Ok(DeleteWait::Deleted);
        };
        if deletion_state(object.meta()) == DeletionState::Live {
            warn!(name, "object reappeared during delete; stopping the wait");
            return Ok(DeleteWait::Unconfirmed);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(DeleteWait::Unconfirmed);
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

/// Whether a failed create was refused by a `ResourceQuota` rather than having
/// gone wrong. The Kubernetes error carries no machine-readable reason for this
/// — the quota admission plugin returns a plain `403 Forbidden` — so the message
/// prefix is what distinguishes it from the other things a 403 means (RBAC, a
/// validating webhook). A misread in either direction is not dangerous: it only
/// changes which honest message the friend gets.
fn is_quota_exceeded(err: &anyhow::Error) -> bool {
    err.downcast_ref::<KubeError>()
        .is_some_and(|err| api_status_code(err) == Some(403) && quota_message(err))
}

fn quota_message(err: &KubeError) -> bool {
    if let KubeError::Api(response) = err {
        response.message.contains("exceeded quota")
    } else {
        false
    }
}

async fn quota_refusal(
    client: &Client,
    namespace: &str,
    entry: &GameCatalogEntry,
) -> CapacityRefusal {
    match check_namespace_capacity(client, namespace, create_admission(entry, None)).await {
        Ok(Err(refusal)) => refusal,
        Ok(Ok(())) => CapacityRefusal::Unspecified,
        Err(err) => {
            warn!(error = ?err, "could not re-read the quota after an admission refusal");
            CapacityRefusal::Unspecified
        }
    }
}

#[cfg(test)]
#[path = "tests/provision.rs"]
mod tests;
