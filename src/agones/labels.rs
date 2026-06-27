use std::collections::BTreeMap;

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
pub(crate) const GAME_KEY: &str = "grizzly-gameservers.bearflinn.com/game";

/// Records the instance name on every object of the trio.
pub(crate) const INSTANCE_KEY: &str = "grizzly-gameservers.bearflinn.com/instance";

/// Selector key Agones auto-applies to each game-server pod; the per-instance
/// `NodePort` Service selects on it with the `GameServer`'s own name as the value.
pub(crate) const GAMESERVER_SELECTOR_KEY: &str = "agones.dev/gameserver";

/// Whether a set of object labels marks it as a shim-provisioned instance.
pub(crate) fn is_managed(labels: Option<&BTreeMap<String, String>>) -> bool {
    labels
        .and_then(|map| map.get(MANAGED_BY_KEY))
        .is_some_and(|value| value == MANAGED_BY_VALUE)
}

#[cfg(test)]
#[path = "tests/labels.rs"]
mod tests;
