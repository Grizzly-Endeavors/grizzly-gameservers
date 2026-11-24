use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::Service;

pub(crate) const MANAGED_BY_KEY: &str = "app.kubernetes.io/managed-by";
pub(crate) const MANAGED_BY_VALUE: &str = "grizzly-gameservers";

pub(crate) const NAME_KEY: &str = "app.kubernetes.io/name";

pub(crate) const GAME_KEY: &str = "grizzly-gameservers.grizzly-endeavors.com/game";

/// Records the instance name on every object of the trio.
pub(crate) const INSTANCE_KEY: &str = "grizzly-gameservers.grizzly-endeavors.com/instance";

/// Records the Discord guild id an instance was created in — the tenant
/// boundary. Every list/lookup/mutation confines itself to servers carrying the
/// caller's guild (the allowlisted cross-guild operator is exempt and sees all).
/// Read back off the surviving Service on `/start`, like [`GAME_KEY`], so scope
/// persists across a stop/start. Absent on pre-scoping and Flux-managed objects.
pub(crate) const GUILD_KEY: &str = "grizzly-gameservers.grizzly-endeavors.com/guild";

pub(crate) const GAMESERVER_SELECTOR_KEY: &str = "agones.dev/gameserver";

pub(crate) fn is_managed(labels: Option<&BTreeMap<String, String>>) -> bool {
    labels
        .and_then(|map| map.get(MANAGED_BY_KEY))
        .is_some_and(|value| value == MANAGED_BY_VALUE)
}

pub(crate) fn label_value<'a>(
    labels: Option<&'a BTreeMap<String, String>>,
    key: &str,
) -> Option<&'a str> {
    labels.and_then(|map| map.get(key)).map(String::as_str)
}

pub(crate) fn service_node_port(service: &Service) -> Option<i32> {
    service
        .spec
        .as_ref()?
        .ports
        .as_ref()?
        .iter()
        .find_map(|port| port.node_port)
}

pub(crate) fn node_port_named(service: &Service, name: &str) -> Option<i32> {
    service
        .spec
        .as_ref()?
        .ports
        .as_ref()?
        .iter()
        .find(|port| port.name.as_deref() == Some(name))
        .and_then(|port| port.node_port)
}

pub(crate) fn all_node_ports(service: &Service) -> Vec<i32> {
    service
        .spec
        .as_ref()
        .and_then(|spec| spec.ports.as_ref())
        .map(|ports| ports.iter().filter_map(|port| port.node_port).collect())
        .unwrap_or_default()
}

pub(crate) fn service_port_names(service: &Service) -> Vec<&str> {
    service
        .spec
        .as_ref()
        .and_then(|spec| spec.ports.as_ref())
        .map(|ports| {
            ports
                .iter()
                .filter_map(|port| port.name.as_deref())
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn service_gameserver_target(service: &Service) -> Option<&str> {
    service
        .spec
        .as_ref()?
        .selector
        .as_ref()?
        .get(GAMESERVER_SELECTOR_KEY)
        .map(String::as_str)
}

#[cfg(test)]
#[path = "tests/labels.rs"]
mod tests;
