use std::collections::{BTreeMap, HashMap};

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

/// The first `NodePort` a Service exposes, if any. Each managed instance's
/// Service leases exactly one, so "first" is the instance's node port.
pub(crate) fn service_node_port(service: &Service) -> Option<i32> {
    service
        .spec
        .as_ref()?
        .ports
        .as_ref()?
        .iter()
        .find_map(|port| port.node_port)
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

/// Map each `NodePort` Service's targeted gameserver to its first `NodePort`, so
/// a gameserver listing can resolve each server's address in one pass.
pub(crate) fn node_ports_by_gameserver(services: &[Service]) -> HashMap<String, i32> {
    services
        .iter()
        .filter_map(|service| {
            Some((
                service_gameserver_target(service)?.to_owned(),
                service_node_port(service)?,
            ))
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/labels.rs"]
mod tests;
