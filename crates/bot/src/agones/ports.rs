//! Per-instance port model. A game declares its port model on the Service
//! template's annotations, which makes the rendered Service **self-describing**:
//! the provisioner reads it to know how many edge-band ports to lease, and
//! `begin_start` and the lister re-derive it straight off the live Service with
//! no catalog access.
//!
//! Two shapes:
//!
//! - **Remap** (no annotations): one leased `NodePort` on the first Service port,
//!   remapped to the container port. The game never advertises a port back to the
//!   client, so `external != container` is invisible. Every single-port game.
//! - **Advertised** (annotated): each named Service port gets its own leased band
//!   port with `nodePort == port == targetPort` (no remap), and the leased number
//!   is injected into the game's env so it binds/advertises the external port.
//!   Required by games like Satisfactory whose client dials the advertised port
//!   and where the edge only forwards the 7000-7010 band.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use k8s_openapi::api::core::v1::Service;
use tracing::warn;

use super::labels::{node_port_named, service_node_port, service_port_names};

/// Annotation naming the Service ports that each get their own leased band port
/// (comma-separated, in lease order). Absence of this key selects the [`PortPlan::Remap`] path.
const ADVERTISED_PORTS_ANNOTATION: &str =
    "grizzly-gameservers.grizzly-endeavors.com/advertised-ports";

/// Annotation naming which advertised port's leased number is reported to the friend.
const FRIEND_FACING_ANNOTATION: &str =
    "grizzly-gameservers.grizzly-endeavors.com/friend-facing-port";

/// Per-port annotation key carrying the comma-separated env vars the leased number
/// is injected into (e.g. `port-env.game: "SERVERGAMEPORT,SUPERVISOR_GAME_PORT"`).
fn port_env_annotation(name: &str) -> String {
    format!("grizzly-gameservers.grizzly-endeavors.com/port-env.{name}")
}

/// How a game's ports map onto leased edge-band ports. See the module docs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PortPlan {
    Remap,
    Advertised(Vec<AdvertisedPort>),
}

/// One advertised Service port: its name (the join key to the Service/GameServer
/// port entries), the env vars its leased number is injected into, and whether
/// it is the friend-facing port reported in Discord.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AdvertisedPort {
    pub(crate) name: String,
    pub(crate) env: Vec<String>,
    pub(crate) friend_facing: bool,
}

/// A [`PortPlan`] with concrete leased port numbers bound in — what the renderer
/// stamps onto the Service and `GameServer` for one instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PortAssignment {
    Remap(i32),
    Advertised(Vec<AssignedPort>),
}

/// An [`AdvertisedPort`] paired with the band port leased to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AssignedPort {
    pub(crate) name: String,
    pub(crate) number: i32,
    pub(crate) env: Vec<String>,
    pub(crate) friend_facing: bool,
}

impl PortAssignment {
    /// The single port reported to the friend in Discord — the sole leased port
    /// for [`Self::Remap`], the friend-facing one for [`Self::Advertised`]. An
    /// Advertised assignment is always non-empty with exactly one friend-facing
    /// port (guaranteed by [`parse_port_plan`]); the `first` fallback only guards
    /// the impossible empty case rather than panicking.
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

/// How many edge-band ports a plan needs leased.
pub(crate) fn ports_needed(plan: &PortPlan) -> usize {
    match plan {
        PortPlan::Remap => 1,
        PortPlan::Advertised(ports) => ports.len(),
    }
}

/// Parse a [`PortPlan`] from a Service's annotations alone. Pure — the port-name
/// cross-check against the Service's `spec.ports` lives in [`port_plan_from_service`].
///
/// # Errors
///
/// Returns an error if the advertise annotation is present but malformed: no
/// ports listed, missing or out-of-list friend-facing port, or a port with no
/// env mapping — a half-configured advertise must fail loud, not silently remap.
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

/// Parse a [`PortPlan`] off a Service, additionally cross-checking that every
/// advertised port names a real `spec.ports` entry to catch an annotation typo
/// before it becomes a render failure.
///
/// # Errors
///
/// Returns an error if the annotations are malformed (see [`parse_port_plan`])
/// or an advertised port has no matching Service port.
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

/// Bind a freshly-parsed plan to the ports leased for it, in list order.
///
/// # Errors
///
/// Returns an error if the number of leased ports does not match what the plan needs.
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

/// Recover the [`PortAssignment`] of a live/surviving Service by re-deriving its
/// plan from annotations and reading the leased numbers back off its ports. Used
/// by `/start` so a stopped instance keeps its exact ports and env injection.
///
/// # Errors
///
/// Returns an error if the Service's plan is malformed, it has no `NodePort`
/// (Remap), or an advertised port has no `NodePort` on the Service.
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

/// The friend-facing `NodePort` of a live Service — the friend-facing advertised
/// port when the Service declares an advertise plan, else its first `NodePort`.
/// Falls back to the first port if the annotations are somehow unparseable so a
/// display path degrades to an address rather than nothing.
pub(crate) fn friend_facing_node_port(service: &Service) -> Option<i32> {
    match port_plan_from_service(service) {
        Ok(PortPlan::Advertised(ports)) => ports
            .iter()
            .find(|port| port.friend_facing)
            .and_then(|port| node_port_named(service, &port.name)),
        // A remap plan resolves to the first NodePort.
        Ok(PortPlan::Remap) => service_node_port(service),
        // Our own rendered services are always parseable, so a parse failure means
        // a hand-edited or foreign annotation. Still degrade to the first NodePort
        // so a display path shows an address rather than nothing — but log it,
        // since this "impossible" case would otherwise silently mangle the address.
        Err(err) => {
            warn!(error = ?err, "unparseable port-advertise annotation; using first NodePort");
            service_node_port(service)
        }
    }
}

/// Map each `NodePort` Service's targeted gameserver to its friend-facing
/// `NodePort`, so a gameserver listing resolves each server's address in one pass.
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

/// Split a comma-separated annotation value into trimmed, non-empty items.
fn split_csv(raw: &str) -> Vec<&str> {
    raw.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .collect()
}

#[cfg(test)]
#[path = "tests/ports.rs"]
mod tests;
