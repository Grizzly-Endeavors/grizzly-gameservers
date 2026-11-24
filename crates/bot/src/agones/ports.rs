use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use k8s_openapi::api::core::v1::Service;
use tracing::warn;

use super::labels::{node_port_named, service_node_port, service_port_names};

const ADVERTISED_PORTS_ANNOTATION: &str =
    "grizzly-gameservers.grizzly-endeavors.com/advertised-ports";

const FRIEND_FACING_ANNOTATION: &str =
    "grizzly-gameservers.grizzly-endeavors.com/friend-facing-port";

fn port_env_annotation(name: &str) -> String {
    format!("grizzly-gameservers.grizzly-endeavors.com/port-env.{name}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PortPlan {
    Remap,
    Advertised(Vec<AdvertisedPort>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AdvertisedPort {
    pub(crate) name: String,
    pub(crate) env: Vec<String>,
    pub(crate) friend_facing: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PortAssignment {
    Remap(i32),
    Advertised(Vec<AssignedPort>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AssignedPort {
    pub(crate) name: String,
    pub(crate) number: i32,
    pub(crate) env: Vec<String>,
    pub(crate) friend_facing: bool,
}

impl PortAssignment {
    pub(crate) fn friend_facing_port(&self) -> i32 {
        match self {
            Self::Remap(number) => *number,
            Self::Advertised(ports) => ports
                .iter()
                .find(|port| port.friend_facing)
                .or_else(|| ports.first())
                .map_or(0, |port| port.number),
        }
    }
}

pub(crate) fn ports_needed(plan: &PortPlan) -> usize {
    match plan {
        PortPlan::Remap => 1,
        PortPlan::Advertised(ports) => ports.len(),
    }
}

pub(crate) fn parse_port_plan(annotations: Option<&BTreeMap<String, String>>) -> Result<PortPlan> {
    let Some(list) = annotations.and_then(|map| map.get(ADVERTISED_PORTS_ANNOTATION)) else {
        return Ok(PortPlan::Remap);
    };
    let names = split_csv(list);
    if names.is_empty() {
        bail!("`{ADVERTISED_PORTS_ANNOTATION}` is set but lists no ports");
    }
    let friend_facing = annotations
        .and_then(|map| map.get(FRIEND_FACING_ANNOTATION))
        .map(String::as_str)
        .with_context(|| format!("advertised game is missing `{FRIEND_FACING_ANNOTATION}`"))?;
    if !names.contains(&friend_facing) {
        bail!(
            "`{FRIEND_FACING_ANNOTATION}` = `{friend_facing}` is not in `{ADVERTISED_PORTS_ANNOTATION}`"
        );
    }

    let mut ports = Vec::with_capacity(names.len());
    for name in names {
        let key = port_env_annotation(name);
        let env_raw = annotations.and_then(|map| map.get(&key)).with_context(|| {
            format!("advertised port `{name}` is missing its `{key}` env mapping")
        })?;
        let env: Vec<String> = split_csv(env_raw).into_iter().map(str::to_owned).collect();
        if env.is_empty() {
            bail!("advertised port `{name}` has an empty env mapping in `{key}`");
        }
        ports.push(AdvertisedPort {
            name: name.to_owned(),
            env,
            friend_facing: name == friend_facing,
        });
    }
    Ok(PortPlan::Advertised(ports))
}

pub(crate) fn port_plan_from_service(service: &Service) -> Result<PortPlan> {
    let plan = parse_port_plan(service.metadata.annotations.as_ref())?;
    if let PortPlan::Advertised(ports) = &plan {
        let names = service_port_names(service);
        for port in ports {
            if !names.iter().any(|name| *name == port.name) {
                bail!(
                    "advertised port `{}` has no matching port in the service template",
                    port.name
                );
            }
        }
    }
    Ok(plan)
}

pub(crate) fn assign(plan: PortPlan, leased: &[i32]) -> Result<PortAssignment> {
    match plan {
        PortPlan::Remap => match leased {
            [number] => Ok(PortAssignment::Remap(*number)),
            other => bail!(
                "remap plan needs exactly one leased port, got {}",
                other.len()
            ),
        },
        PortPlan::Advertised(ports) => {
            if ports.len() != leased.len() {
                bail!(
                    "advertised plan needs {} leased ports, got {}",
                    ports.len(),
                    leased.len()
                );
            }
            let assigned = ports
                .into_iter()
                .zip(leased.iter())
                .map(|(port, &number)| AssignedPort {
                    name: port.name,
                    number,
                    env: port.env,
                    friend_facing: port.friend_facing,
                })
                .collect();
            Ok(PortAssignment::Advertised(assigned))
        }
    }
}

pub(crate) fn assignment_from_service(service: &Service) -> Result<PortAssignment> {
    match port_plan_from_service(service)? {
        PortPlan::Remap => {
            let number = service_node_port(service).context("managed service has no nodeport")?;
            Ok(PortAssignment::Remap(number))
        }
        PortPlan::Advertised(ports) => {
            let assigned = ports
                .into_iter()
                .map(|port| {
                    let number = node_port_named(service, &port.name).with_context(|| {
                        format!(
                            "advertised port `{}` has no nodeport on the service",
                            port.name
                        )
                    })?;
                    Ok(AssignedPort {
                        name: port.name,
                        number,
                        env: port.env,
                        friend_facing: port.friend_facing,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(PortAssignment::Advertised(assigned))
        }
    }
}

pub(crate) fn friend_facing_node_port(service: &Service) -> Option<i32> {
    match port_plan_from_service(service) {
        Ok(PortPlan::Advertised(ports)) => ports
            .iter()
            .find(|port| port.friend_facing)
            .and_then(|port| node_port_named(service, &port.name)),
        Ok(PortPlan::Remap) => service_node_port(service),
        Err(err) => {
            warn!(error = ?err, "unparseable port-advertise annotation; using first NodePort");
            service_node_port(service)
        }
    }
}

pub(crate) fn node_ports_by_gameserver(
    services: &[Service],
) -> std::collections::HashMap<String, i32> {
    services
        .iter()
        .filter_map(|service| {
            Some((
                super::labels::service_gameserver_target(service)?.to_owned(),
                friend_facing_node_port(service)?,
            ))
        })
        .collect()
}

fn split_csv(raw: &str) -> Vec<&str> {
    raw.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .collect()
}

#[cfg(test)]
#[path = "tests/ports.rs"]
mod tests;
