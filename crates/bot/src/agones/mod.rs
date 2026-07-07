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
mod supervisor;
mod supervisor_fs;
mod types;

pub(crate) use catalog::{GameCatalog, load_catalog};
pub(crate) use client::list_active_servers;
pub(crate) use naming::{build_instance_name, now_entropy};
pub(crate) use provision::{
    CreateOutcome, DestroyOutcome, ProvisionOutcome, ShutdownOutcome, StartBegin, StartOutcome,
    begin_start, destroy_instance, list_instance_names, provision_instance, shutdown_instance,
    wait_for_instance_ready,
};
pub(crate) use supervisor::{
    RuntimeState, SupervisorOutcome, instance_runtime_state, supervisor_restart, supervisor_start,
    supervisor_stop,
};
pub(crate) use supervisor_fs::{
    FsOutcome, supervisor_list_files, supervisor_read_file, supervisor_read_logs,
    supervisor_restore_file, supervisor_send_command, supervisor_write_file,
};
pub(crate) use types::ServerSummary;
