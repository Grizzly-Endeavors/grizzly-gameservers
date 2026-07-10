use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use k8s_openapi::api::core::v1::{PersistentVolumeClaim, Service};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::core::DynamicObject;
use serde_json::Value;

use super::catalog::GameCatalogEntry;
use super::labels::{
    GAME_KEY, GAMESERVER_SELECTOR_KEY, GUILD_KEY, INSTANCE_KEY, MANAGED_BY_KEY, MANAGED_BY_VALUE,
    NAME_KEY,
};
use super::naming::pvc_name;
use super::ports::{AssignedPort, PortAssignment};
use crate::domain::{GameId, GuildId, InstanceName};

/// Everything the renderer needs to stamp a catalog template into a concrete
/// per-world instance. `ports` carries the leased edge-band port(s): a single
/// remapped port for most games, or the advertised game/messaging ports whose
/// numbers are stamped onto the Service and injected into the game's env.
#[derive(Clone, Debug)]
pub(crate) struct InstanceIdentity {
    pub(crate) name: InstanceName,
    pub(crate) game: GameId,
    pub(crate) namespace: String,
    pub(crate) ports: PortAssignment,
    /// Discord guild id that owns this instance (the [`GUILD_KEY`] label).
    /// Empty leaves the label off — for pre-scoping instances whose surviving
    /// Service carries no guild, so a cold `/start` doesn't stamp a bogus one.
    pub(crate) guild: GuildId,
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
    obj.metadata.name = Some(identity.name.as_str().to_owned());
    obj.metadata.namespace = Some(identity.namespace.clone());
    apply_labels(&mut obj.metadata.labels, identity);
    rebind_claim(&mut obj.data, &pvc_name(identity.name.as_str()))?;
    if identity.start_paused {
        upsert_container_env(&mut obj.data, START_PAUSED_ENV, "true")?;
    }
    if let PortAssignment::Advertised(ports) = &identity.ports {
        apply_advertised_ports(&mut obj.data, ports)?;
    }
    Ok(obj)
}

/// For an advertising game, point every declared game/messaging port at its
/// leased edge-band number: rewrite the matching container ports (Agones
/// `spec.ports` and the pod template's container ports) and inject the leased
/// number into the game's env so the process binds and advertises that exact
/// external port (Satisfactory's game port cannot be redirected — external must
/// equal the number the server binds).
fn apply_advertised_ports(data: &mut Value, ports: &[AssignedPort]) -> Result<()> {
    for assigned in ports {
        rewrite_container_ports(data, &assigned.name, assigned.number);
        for env in &assigned.env {
            upsert_container_env(data, env, &assigned.number.to_string())?;
        }
    }
    Ok(())
}

/// Set `containerPort` to `number` on every port named `name`, across both the
/// Agones `spec.ports` and each pod container's `ports`. A port the template
/// doesn't declare is simply not found — the port-name cross-check in
/// `port_plan_from_service` already rejects an advertised port with no entry.
fn rewrite_container_ports(data: &mut Value, name: &str, number: i32) {
    if let Some(ports) = data
        .get_mut("spec")
        .and_then(|spec| spec.get_mut("ports"))
        .and_then(Value::as_array_mut)
    {
        set_named_container_port(ports, name, number);
    }
    if let Some(containers) = data
        .get_mut("spec")
        .and_then(|spec| spec.get_mut("template"))
        .and_then(|template| template.get_mut("spec"))
        .and_then(|pod_spec| pod_spec.get_mut("containers"))
        .and_then(Value::as_array_mut)
    {
        for container in containers.iter_mut() {
            if let Some(ports) = container.get_mut("ports").and_then(Value::as_array_mut) {
                set_named_container_port(ports, name, number);
            }
        }
    }
}

fn set_named_container_port(ports: &mut [Value], name: &str, number: i32) {
    for port in ports.iter_mut() {
        if port.get("name").and_then(Value::as_str) == Some(name)
            && let Some(object) = port.as_object_mut()
        {
            object.insert("containerPort".to_owned(), Value::Number(number.into()));
        }
    }
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
    svc.metadata.name = Some(identity.name.as_str().to_owned());
    svc.metadata.namespace = Some(identity.namespace.clone());
    apply_labels(&mut svc.metadata.labels, identity);

    let spec = svc.spec.as_mut().context("service template has no spec")?;
    spec.selector.get_or_insert_with(BTreeMap::new).insert(
        GAMESERVER_SELECTOR_KEY.to_owned(),
        identity.name.as_str().to_owned(),
    );

    match &identity.ports {
        PortAssignment::Remap(number) => {
            let first_port = spec
                .ports
                .as_mut()
                .and_then(|ports| ports.first_mut())
                .context("service template has no ports to expose")?;
            first_port.node_port = Some(*number);
        }
        PortAssignment::Advertised(ports) => {
            let svc_ports = spec
                .ports
                .as_mut()
                .context("service template has no ports to expose")?;
            for assigned in ports {
                let target = svc_ports
                    .iter_mut()
                    .find(|port| port.name.as_deref() == Some(&assigned.name))
                    .with_context(|| {
                        format!("service template has no port named `{}`", assigned.name)
                    })?;
                // No remap: nodePort == port == targetPort == the advertised port
                // the game binds, so the number a client dials reaches the pod.
                target.node_port = Some(assigned.number);
                target.port = assigned.number;
                target.target_port = Some(IntOrString::Int(assigned.number));
            }
        }
    }

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
    pvc.metadata.name = Some(pvc_name(identity.name.as_str()));
    pvc.metadata.namespace = Some(identity.namespace.clone());
    apply_labels(&mut pvc.metadata.labels, identity);
    Ok(pvc)
}

fn apply_labels(labels: &mut Option<BTreeMap<String, String>>, identity: &InstanceIdentity) {
    let map = labels.get_or_insert_with(BTreeMap::new);
    map.insert(NAME_KEY.to_owned(), identity.game.as_str().to_owned());
    map.insert(MANAGED_BY_KEY.to_owned(), MANAGED_BY_VALUE.to_owned());
    map.insert(GAME_KEY.to_owned(), identity.game.as_str().to_owned());
    map.insert(INSTANCE_KEY.to_owned(), identity.name.as_str().to_owned());
    if !identity.guild.as_str().is_empty() {
        map.insert(GUILD_KEY.to_owned(), identity.guild.as_str().to_owned());
    }
}

/// Repoint every `persistentVolumeClaim` volume in the pod template at `pvc`.
/// Errors if the template exposes no such volume — a game with persistent state
/// must declare one, and silently creating a server with no world storage would
/// be a data-loss surprise. Catalog templates declare exactly one claim today;
/// if a future template carried two, both would collapse onto this single PVC,
/// so a multi-claim template must revisit this.
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
