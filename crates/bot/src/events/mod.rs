//! The product event log: a permanent, append-only record of what friends
//! actually do with their servers.
//!
//! This is the Postgres half of the two-plane data collection design
//! (`docs/archive/data-collection/design.md`). Langfuse holds the LLM-interaction
//! plane — traces, generations, tool calls, outcome scores. This plane holds the
//! product plane: lifecycle actions, backups, escalations, and acquisition, in
//! the shapes SQL answers well (distinct active guilds per week, time from join
//! to first server, archive-and-never-recover as the churn signal).
//!
//! Five pieces: [`catalog`] is the closed vocabulary that lands in the columns,
//! [`mapping`] turns each orchestration outcome into that vocabulary, [`store`]
//! is the sink for every table, and [`sampler`] and [`resources`] are the
//! background writers of the occupancy and cost time series respectively.
//! Events are emitted at the layer that *executes* an
//! action rather than inside the agones orchestration functions, because those
//! functions double as internal building blocks — an archive tears a server down
//! through the same `destroy_instance` a friend does — and because a create's
//! terminal outcome is its readiness resolution, which only the caller awaits.

mod catalog;
pub(crate) mod mapping;
mod resources;
mod sampler;
mod store;

pub(crate) use catalog::{Actor, Attribution, EventKind, ProductEvent, Surface};
pub(crate) use resources::{ReportedUnlabelledPvcs, ResourceSample, run_resource_cycle};
pub(crate) use sampler::{SamplerCtx, run_occupancy_cycle};
pub(crate) use store::EventLog;
