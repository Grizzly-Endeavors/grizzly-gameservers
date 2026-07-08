//! Kubernetes/Agones access: the per-game catalog, listing and lifecycle
//! (create/start/stop/restart/shutdown/destroy) of `GameServer` instances, and
//! the client for the in-pod supervisor's control API. Everything here talks
//! to the cluster; the Discord-facing layer in `crate::discord` composes these
//! into commands and Gary's tools.

mod catalog;
mod client;
mod instance;
mod labels;
mod naming;
mod provision;
mod scope;
mod supervisor;
mod supervisor_fs;
mod types;

pub(crate) use catalog::{GameCatalog, load_catalog};
pub(crate) use client::{BackupTarget, list_active_servers, list_backup_targets};
pub(crate) use naming::{build_instance_name, now_entropy};
pub(crate) use provision::{
    CreateOutcome, DestroyOutcome, ProvisionOutcome, ShutdownOutcome, StartBegin, StartOutcome,
    begin_start, destroy_instance, list_instance_names, provision_instance,
    provision_paused_instance, shutdown_instance, wait_for_instance_ready,
};
pub(crate) use scope::{ScopeVerdict, ServerScope, guild_of, verify_scope};
pub(crate) use supervisor::{
    ControlReady, PodTarget, ReadyWait, RuntimeState, SupervisorOutcome, instance_runtime_state,
    resolve_managed_pod, supervisor_restart, supervisor_start, supervisor_stop,
    wait_for_control_reachable, wait_for_ready,
};
pub(crate) use supervisor_fs::{
    EditOutcome, FsOutcome, Replacement, supervisor_announce, supervisor_edit_file,
    supervisor_list_files, supervisor_read_file, supervisor_read_logs, supervisor_restore_file,
    supervisor_send_command, supervisor_write_file,
};
pub(crate) use types::ServerSummary;
