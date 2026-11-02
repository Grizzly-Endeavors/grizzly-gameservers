//! Kubernetes/Agones access: the per-game catalog, listing and lifecycle
//! (create/start/stop/restart/shutdown/destroy) of `GameServer` instances, and
//! the client for the in-pod supervisor's control API. Everything here talks
//! to the cluster; the Discord-facing layer in `crate::discord` composes these
//! into commands and Gary's tools.

mod capacity;
mod catalog;
mod client;
mod control_client;
mod directory;
mod env_overlay;
mod instance;
mod labels;
mod mod_registry;
mod naming;
mod ports;
mod provision;
mod resize;
mod scope;
mod stranded;
mod supervisor;
mod supervisor_fs;
mod template;
mod types;

pub(crate) use catalog::{GameCatalog, load_catalog};
pub(crate) use client::{LiveInstance, guild_usage, list_active_servers, list_live_instances};
pub(crate) use control_client::ControlClient;
pub(crate) use directory::{FixedServer, FixedServers, ServerDirectory, Servers};
pub(crate) use env_overlay::EnvOverlay;
pub(crate) use instance::catalog_sizing;
pub(crate) use labels::{
    GAME_KEY, GAMESERVER_SELECTOR_KEY, GUILD_KEY, INSTANCE_KEY, MANAGED_BY_KEY, MANAGED_BY_VALUE,
    label_value,
};
pub(crate) use mod_registry::ModRegistry;
// Only the catalog drift guards in `crate::tests::config` read the strict
// parsers — every runtime path goes through the lenient `*_or_warn` wrappers
// beside them — so exporting these unconditionally would be an unused import.
#[cfg(test)]
pub(crate) use env_overlay::parse_env_overlay_from_template;
#[cfg(test)]
pub(crate) use mod_registry::{parse_mod_registry_from_template, parse_mods_file_from_template};
pub(crate) use naming::{build_instance_name, now_entropy, validate_world_name};
pub(crate) use provision::{
    CreateOutcome, DestroyOutcome, ProvisionOutcome, ProvisionSpec, ShutdownOutcome, StartBegin,
    StartOutcome, begin_start, destroy_instance, list_instance_names, provision_instance,
    provision_paused_instance, shutdown_instance, wait_for_instance_ready,
};
pub(crate) use resize::{ResizeOutcome, resize_instance};
pub(crate) use scope::{InstanceOwner, ScopeVerdict, ServerScope, owner_in_cluster};
pub(crate) use stranded::{StrandedCtx, run_stranded_cycle};
pub(crate) use supervisor::{
    ControlReady, PodTarget, ReadyWait, RuntimeState, StatusPoll, SupervisorOutcome,
    instance_runtime_state, supervisor_restart, supervisor_start, supervisor_status,
    supervisor_stop, wait_for_control_reachable, wait_for_ready, wait_for_ready_within,
};
// Only the drift guard in `crate::tests::config` reads this — the runtime callers
// are inside `supervisor` itself — so exporting it unconditionally would be an
// unused import in a normal build.
#[cfg(test)]
pub(crate) use supervisor::CONTROL_MUTATION_TIMEOUT;
pub(crate) use supervisor_fs::{
    EditOutcome, FsOutcome, ReadWindow, Replacement, supervisor_announce, supervisor_edit_file,
    supervisor_list_files, supervisor_occupancy, supervisor_read_file, supervisor_read_logs,
    supervisor_restore_file, supervisor_run_shell, supervisor_send_command, supervisor_write_file,
};
pub(crate) use types::ServerSummary;
// Named only by the discord/agent render test helpers that rebuild a
// `ServerSummary`; production code reaches state through `ServerSummary`'s
// `is_online`/`Display`, so the facade export would otherwise read as unused.
#[cfg(test)]
pub(crate) use types::ServerState;
