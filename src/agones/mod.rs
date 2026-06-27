mod catalog;
mod client;
mod instance;
mod labels;
mod naming;
mod provision;
mod types;

pub(crate) use catalog::{GameCatalog, load_catalog};
pub(crate) use client::list_active_servers;
pub(crate) use naming::build_instance_name;
pub(crate) use provision::{
    CreateOutcome, ProvisionOutcome, RemoveOutcome, StartBegin, StartOutcome, StopOutcome,
    begin_start, list_instance_names, provision_instance, remove_instance, stop_instance,
    wait_for_instance_ready,
};
pub(crate) use types::ServerSummary;
