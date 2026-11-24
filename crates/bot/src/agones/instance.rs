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
use crate::events::{parse_bytes, parse_cpu_millicores};
use crate::sizing::Sizing;

#[derive(Clone, Debug)]
pub(crate) struct InstanceIdentity {
    pub(crate) name: InstanceName,
    pub(crate) game: GameId,
    pub(crate) namespace: String,
    pub(crate) ports: PortAssignment,
    pub(crate) guild: GuildId,
    pub(crate) start_paused: bool,
    pub(crate) sizing: Option<Sizing>,
}

const START_PAUSED_ENV: &str = "SUPERVISOR_START_PAUSED";

pub(crate) fn render_gameserver(
    entry: &GameCatalogEntry,
    identity: &InstanceIdentity,
) -> Result<DynamicObject> {
    let mut obj: DynamicObject = serde_saphyr::from_str(&entry.gameserver_yaml)
        .with_context(|| format!("failed to parse gameserver template for game {}", entry.id))?;
    obj.metadata.name = Some(identity.name.as_str().to_owned());
    obj.metadata.namespace = Some(identity.namespace.clone());
    apply_labels(&mut obj.metadata.labels, identity);
    rebind_claim(&mut obj.data, &pvc_name(identity.name.as_str()))?;
    if identity.start_paused {
        upsert_container_env(&mut obj.data, START_PAUSED_ENV, "true")?;
    }
    if let Some(sizing) = identity.sizing {
        set_container_resources(&mut obj.data, sizing)?;
    }
    if let PortAssignment::Advertised(ports) = &identity.ports {
        apply_advertised_ports(&mut obj.data, ports)?;
    }
    Ok(obj)
}

fn apply_advertised_ports(data: &mut Value, ports: &[AssignedPort]) -> Result<()> {
    for assigned in ports {
        rewrite_container_ports(data, &assigned.name, assigned.number);
        for env in &assigned.env {
            upsert_container_env(data, env, &assigned.number.to_string())?;
        }
    }
    Ok(())
}

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

fn set_container_resources(data: &mut Value, sizing: Sizing) -> Result<()> {
    let quantities = sizing.as_quantities();
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
        object.insert(
            "resources".to_owned(),
            serde_json::json!({
                "requests": {
                    "cpu": quantities.cpu_request,
                    "memory": quantities.mem_request,
                },
                "limits": {
                    "cpu": quantities.cpu_limit,
                    "memory": quantities.mem_limit,
                },
            }),
        );
    }
    Ok(())
}

pub(crate) fn catalog_sizing(entry: &GameCatalogEntry) -> Option<Sizing> {
    let template: Value = serde_saphyr::from_str(&entry.gameserver_yaml).ok()?;
    let spec = template.get("spec")?;
    let game_container = spec.get("container").and_then(Value::as_str)?;
    let resources = spec
        .get("template")?
        .get("spec")?
        .get("containers")?
        .as_array()?
        .iter()
        .find(|container| container.get("name").and_then(Value::as_str) == Some(game_container))?
        .get("resources")?;
    let quantity = |side: &str, key: &str| {
        resources
            .get(side)
            .and_then(|declared| declared.get(key))
            .and_then(quantity_text)
    };
    Some(Sizing {
        cpu_request_millicores: parse_cpu_millicores(&quantity("requests", "cpu")?)?,
        mem_request_bytes: parse_bytes(&quantity("requests", "memory")?)?,
        cpu_limit_millicores: parse_cpu_millicores(&quantity("limits", "cpu")?)?,
        mem_limit_bytes: parse_bytes(&quantity("limits", "memory")?)?,
    })
}

fn quantity_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) => None,
    }
}

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
                entry.remove("valueFrom");
            }
            None => entries.push(serde_json::json!({ "name": key, "value": value })),
        }
    }
    Ok(())
}

pub(crate) fn render_service(
    entry: &GameCatalogEntry,
    identity: &InstanceIdentity,
) -> Result<Service> {
    let mut svc: Service = serde_saphyr::from_str(&entry.service_yaml)
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
                target.node_port = Some(assigned.number);
                target.port = assigned.number;
                target.target_port = Some(IntOrString::Int(assigned.number));
            }
        }
    }

    Ok(svc)
}

pub(crate) fn render_pvc(
    entry: &GameCatalogEntry,
    identity: &InstanceIdentity,
) -> Result<PersistentVolumeClaim> {
    let mut pvc: PersistentVolumeClaim = serde_saphyr::from_str(&entry.pvc_yaml)
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
