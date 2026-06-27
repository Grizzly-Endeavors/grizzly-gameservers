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
    CreateOutcome, RemoveOutcome, StartOutcome, StopOutcome, create_instance, list_instance_names,
    remove_instance, start_instance, stop_instance,
};
pub(crate) use types::ServerSummary;
