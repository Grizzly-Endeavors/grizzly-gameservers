use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use k8s_openapi::api::core::v1::{PersistentVolumeClaim, Service};
use kube::core::DynamicObject;
use serde_json::Value;

use super::catalog::GameCatalogEntry;
use super::labels::{
    CHANNEL_KEY, GAME_KEY, GAMESERVER_SELECTOR_KEY, INSTANCE_KEY, MANAGED_BY_KEY, MANAGED_BY_VALUE,
    NAME_KEY,
};
use super::naming::pvc_name;

/// Everything the renderer needs to stamp a catalog template into a concrete
/// per-world instance. `node_port` is only consumed by the Service render.
#[derive(Clone, Debug)]
pub(crate) struct InstanceIdentity {
    pub(crate) name: String,
    pub(crate) game: String,
    pub(crate) namespace: String,
    pub(crate) node_port: i32,
    /// Discord channel id that owns this instance (the [`CHANNEL_KEY`] label).
    /// Empty leaves the label off — for pre-scoping instances whose surviving
    /// Service carries no channel, so a cold `/start` doesn't stamp a bogus one.
    pub(crate) channel: String,
    /// Hold the game process down at boot (inject `SUPERVISOR_START_PAUSED`) so the
    /// bot can seed `/data` from an archive before the first launch. Only set by
    /// recover-from-archive; a normal create/start leaves it `false`.
    pub(crate) start_paused: bool,
}

/// Env var the supervisor reads to boot paused; injected into the game container
/// when [`InstanceIdentity::start_paused`] is set. Matches the supervisor's own
/// `SUPERVISOR_START_PAUSED`.
const START_PAUSED_ENV: &str = "SUPERVISOR_START_PAUSED";

/// Render the `GameServer` for an instance: parse the catalog template, then
/// override identity (name, namespace, labels) and rebind its data volume to the
/// per-instance PVC. The rest of the pod spec is left exactly as authored.
///
/// # Errors
///
/// Returns an error if the template is not valid YAML or has no
/// `persistentVolumeClaim` volume to rebind.
pub(crate) fn render_gameserver(
    entry: &GameCatalogEntry,
    identity: &InstanceIdentity,
) -> Result<DynamicObject> {
    let mut obj: DynamicObject = serde_yaml_ng::from_str(&entry.gameserver_yaml)
        .with_context(|| format!("failed to parse gameserver template for game {}", entry.id))?;
    obj.metadata.name = Some(identity.name.clone());
    obj.metadata.namespace = Some(identity.namespace.clone());
    apply_labels(&mut obj.metadata.labels, identity);
    rebind_claim(&mut obj.data, &pvc_name(&identity.name))?;
    if identity.start_paused {
        upsert_container_env(&mut obj.data, START_PAUSED_ENV, "true")?;
    }
    Ok(obj)
}

/// Set (or add) an env var on every container in the pod template. The catalog
/// template defines only the game container — Agones injects its SDK sidecar at
/// admission, not here — so this reaches exactly the supervisor's container.
fn upsert_container_env(data: &mut Value, key: &str, value: &str) -> Result<()> {
    let containers = data
        .get_mut("spec")
        .and_then(|spec| spec.get_mut("template"))
        .and_then(|template| template.get_mut("spec"))
        .and_then(|pod_spec| pod_spec.get_mut("containers"))
        .and_then(Value::as_array_mut)
        .context("gameserver template has no spec.template.spec.containers")?;
    for container in containers.iter_mut() {
        let Some(object) = container.as_object_mut() else {
            continue;
        };
        let env = object
            .entry("env")
            .or_insert_with(|| Value::Array(Vec::new()));
        let entries = env
            .as_array_mut()
            .context("gameserver container `env` is not a list")?;
        match entries
            .iter_mut()
            .find(|entry| entry.get("name").and_then(Value::as_str) == Some(key))
        {
            Some(existing) => {
                let entry = existing
                    .as_object_mut()
                    .context("gameserver container env entry is not a map")?;
                entry.insert("value".to_owned(), Value::String(value.to_owned()));
                // Drop any valueFrom so the literal value we set is authoritative.
                entry.remove("valueFrom");
            }
            None => entries.push(serde_json::json!({ "name": key, "value": value })),
        }
    }
    Ok(())
}

/// Render the per-instance `NodePort` Service: override identity, point the
/// `agones.dev/gameserver` selector at this instance's `GameServer` name, and set
/// the leased `NodePort`.
///
/// # Errors
///
/// Returns an error if the template is not valid YAML, has no spec, or has no
/// ports to assign the `NodePort` to.
pub(crate) fn render_service(
    entry: &GameCatalogEntry,
    identity: &InstanceIdentity,
) -> Result<Service> {
    let mut svc: Service = serde_yaml_ng::from_str(&entry.service_yaml)
        .with_context(|| format!("failed to parse service template for game {}", entry.id))?;
    svc.metadata.name = Some(identity.name.clone());
    svc.metadata.namespace = Some(identity.namespace.clone());
    apply_labels(&mut svc.metadata.labels, identity);

    let spec = svc.spec.as_mut().context("service template has no spec")?;
    spec.selector
        .get_or_insert_with(BTreeMap::new)
        .insert(GAMESERVER_SELECTOR_KEY.to_owned(), identity.name.clone());

    let first_port = spec
        .ports
        .as_mut()
        .and_then(|ports| ports.first_mut())
        .context("service template has no ports to expose")?;
    first_port.node_port = Some(identity.node_port);

    Ok(svc)
}

/// Render the per-instance PVC: override identity. The storage class and size
/// from the template are left untouched.
///
/// # Errors
///
/// Returns an error if the template is not valid YAML.
pub(crate) fn render_pvc(
    entry: &GameCatalogEntry,
    identity: &InstanceIdentity,
) -> Result<PersistentVolumeClaim> {
    let mut pvc: PersistentVolumeClaim = serde_yaml_ng::from_str(&entry.pvc_yaml)
        .with_context(|| format!("failed to parse pvc template for game {}", entry.id))?;
    pvc.metadata.name = Some(pvc_name(&identity.name));
    pvc.metadata.namespace = Some(identity.namespace.clone());
    apply_labels(&mut pvc.metadata.labels, identity);
    Ok(pvc)
}

fn apply_labels(labels: &mut Option<BTreeMap<String, String>>, identity: &InstanceIdentity) {
    let map = labels.get_or_insert_with(BTreeMap::new);
    map.insert(NAME_KEY.to_owned(), identity.game.clone());
    map.insert(MANAGED_BY_KEY.to_owned(), MANAGED_BY_VALUE.to_owned());
    map.insert(GAME_KEY.to_owned(), identity.game.clone());
    map.insert(INSTANCE_KEY.to_owned(), identity.name.clone());
    if !identity.channel.is_empty() {
        map.insert(CHANNEL_KEY.to_owned(), identity.channel.clone());
    }
}

/// Repoint every `persistentVolumeClaim` volume in the pod template at `pvc`.
/// Errors if the template exposes no such volume — a game with persistent state
/// must declare one, and silently creating a server with no world storage would
/// be a data-loss surprise.
fn rebind_claim(data: &mut Value, pvc: &str) -> Result<()> {
    let volumes = data
        .get_mut("spec")
        .and_then(|spec| spec.get_mut("template"))
        .and_then(|template| template.get_mut("spec"))
        .and_then(|pod_spec| pod_spec.get_mut("volumes"))
        .and_then(Value::as_array_mut)
        .context("gameserver template has no spec.template.spec.volumes")?;

    let mut rebound = false;
    for volume in volumes.iter_mut() {
        if let Some(claim) = volume
            .get_mut("persistentVolumeClaim")
            .and_then(|claim| claim.get_mut("claimName"))
        {
            *claim = Value::String(pvc.to_owned());
            rebound = true;
        }
    }

    if !rebound {
        bail!("gameserver template has no persistentVolumeClaim volume to rebind");
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/instance.rs"]
mod tests;
