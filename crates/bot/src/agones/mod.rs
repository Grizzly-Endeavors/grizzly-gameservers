mod catalog;
mod client;
mod instance;
mod labels;
mod naming;
mod provision;
mod supervisor;
mod types;

pub(crate) use catalog::{GameCatalog, load_catalog};
pub(crate) use client::list_active_servers;
pub(crate) use naming::build_instance_name;
pub(crate) use provision::{
    CreateOutcome, KillOutcome, ProvisionOutcome, RemoveOutcome, StartBegin, StartOutcome,
    begin_start, kill_instance, list_instance_names, provision_instance, remove_instance,
    wait_for_instance_ready,
};
pub(crate) use supervisor::{
    RuntimeState, SupervisorOutcome, instance_runtime_state, supervisor_restart, supervisor_start,
    supervisor_stop,
};
pub(crate) use types::ServerSummary;
