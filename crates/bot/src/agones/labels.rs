use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::Service;

/// Label key + value that marks an object as a shim-provisioned per-world
/// instance. The Flux-managed singleton game servers do **not** carry this, so
/// it is the guard that stops the bot from deleting or restarting objects the
/// `GitOps` reconciler owns.
pub(crate) const MANAGED_BY_KEY: &str = "app.kubernetes.io/managed-by";
pub(crate) const MANAGED_BY_VALUE: &str = "grizzly-gameservers";

/// Conventional name label (`app.kubernetes.io/name`), set to the game id.
pub(crate) const NAME_KEY: &str = "app.kubernetes.io/name";

/// Records which catalog game an instance was created from. Read back off the
/// Service on `/start`, which is the only object that survives a `/stop`.
pub(crate) const GAME_KEY: &str = "grizzly-gameservers.grizzly-endeavors.com/game";

/// Records the instance name on every object of the trio.
pub(crate) const INSTANCE_KEY: &str = "grizzly-gameservers.grizzly-endeavors.com/instance";

/// Records the Discord guild id an instance was created in — the tenant
/// boundary. Every list/lookup/mutation confines itself to servers carrying the
/// caller's guild (the allowlisted cross-guild operator is exempt and sees all).
/// Read back off the surviving Service on `/start`, like [`GAME_KEY`], so scope
/// persists across a stop/start. Absent on pre-scoping and Flux-managed objects.
pub(crate) const GUILD_KEY: &str = "grizzly-gameservers.grizzly-endeavors.com/guild";

/// Selector key Agones auto-applies to each game-server pod; the per-instance
/// `NodePort` Service selects on it with the `GameServer`'s own name as the value.
pub(crate) const GAMESERVER_SELECTOR_KEY: &str = "agones.dev/gameserver";

/// Whether a set of object labels marks it as a shim-provisioned instance.
pub(crate) fn is_managed(labels: Option<&BTreeMap<String, String>>) -> bool {
    labels
        .and_then(|map| map.get(MANAGED_BY_KEY))
        .is_some_and(|value| value == MANAGED_BY_VALUE)
}

/// Read a single label value off an object's label map, if present. The one home
/// for the `labels.as_ref().and_then(|m| m.get(KEY))` idiom.
pub(crate) fn label_value<'a>(
    labels: Option<&'a BTreeMap<String, String>>,
    key: &str,
) -> Option<&'a str> {
    labels.and_then(|map| map.get(key)).map(String::as_str)
}

/// The first `NodePort` a Service exposes, if any. A single-port (remap) instance
/// leases exactly one, so "first" is its node port; multi-port instances resolve
/// their friend-facing port through [`super::ports::friend_facing_node_port`].
pub(crate) fn service_node_port(service: &Service) -> Option<i32> {
    service
        .spec
        .as_ref()?
        .ports
        .as_ref()?
        .iter()
        .find_map(|port| port.node_port)
}

/// The `NodePort` of the Service port named `name`, if any. The join key for
/// resolving a specific advertised port's leased number off a live Service.
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

/// Every `NodePort` a Service exposes. A multi-port advertised instance leases
/// one per port, so occupancy accounting must count them all, not just the first.
pub(crate) fn all_node_ports(service: &Service) -> Vec<i32> {
    service
        .spec
        .as_ref()
        .and_then(|spec| spec.ports.as_ref())
        .map(|ports| ports.iter().filter_map(|port| port.node_port).collect())
        .unwrap_or_default()
}

/// The names of the ports a Service exposes, for cross-checking an advertise
/// plan against the ports actually declared.
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

/// The `GameServer` a `NodePort` Service targets, via its `agones.dev/gameserver`
/// selector — the join key between a Service and the pod behind it.
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
