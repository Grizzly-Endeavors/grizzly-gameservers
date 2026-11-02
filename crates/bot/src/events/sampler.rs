//! The occupancy sampler: the time-series writer of the product plane.
//!
//! Every tick it enumerates the managed instances with a live pod, asks each
//! one's supervisor two questions — how many players are on (`/occupancy`) and
//! how many times the game process has crashed and been relaunched (`/status`) —
//! and appends a row per instance. Integrating those rows gives playtime per
//! guild; taking their maximum gives peak concurrency; differencing the restart
//! counter gives crash frequency. None of that is reconstructable after the
//! fact, which is why sampling runs whether or not anyone is looking.
//!
//! Two invariants shape the code below and are what the tests pin:
//!
//! - **A null player count means *unknown*, never zero.** A game with no working
//!   occupancy probe, or a console that wasn't reachable this tick, records null.
//!   Reading null as "empty" would manufacture idle time that never happened —
//!   the same trap `crate::defer::condition` already guards against.
//! - **A paused instance gets no sample row.** Its process is down, so it has
//!   no players by definition; rows for it would be pure null noise. Skipping
//!   is also what makes stopping a server stop its samples.
//!
//! The watchdog *observations* deliberately cover more than the rows: every
//! listed instance yields one, running or not, because crash escalation labels
//! the process `stopped` (the same label as an intentional pause) and tears the
//! pod down shortly after — an escalated crash that was only observed through
//! the running filter would hide itself. See ADR-016.
//!
//! Nothing here reaches the cluster in a new way: the enumeration and both
//! supervisor calls are the ones the backup cycle and the deferred-task watcher
//! already use, over egress that already exists.

use kube::Client;
use tracing::{debug, error};

use super::store::EventLog;
use crate::agones::{
    FsOutcome, LiveInstance, StatusPoll, list_live_instances, supervisor_occupancy,
    supervisor_status,
};
use crate::domain::{GameId, GuildId, InstanceName};
use crate::watchdog::{PollObservation, StatusObservation};

/// One instance's reading at one moment, bound for the `occupancy_samples` table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OccupancySample {
    pub(crate) instance: InstanceName,
    pub(crate) game: Option<GameId>,
    pub(crate) guild: Option<GuildId>,
    /// Connected players, or `None` for *unknown* — never a stand-in for zero.
    pub(crate) players: Option<u32>,
    /// Unexpected exits the supervisor has relaunched from, counted since the pod
    /// started. A value, not a delta: the derivation layer differences successive
    /// rows, and a pod restart resetting it to zero is itself the signal.
    pub(crate) restarts: u32,
}

/// Cluster and store handles one sampler pass needs, mirroring the shape the
/// backup and stranded cycles take.
pub(crate) struct SamplerCtx<'a> {
    pub(crate) client: &'a Client,
    pub(crate) http: &'a crate::agones::ControlClient,
    pub(crate) namespace: &'a str,
    pub(crate) control_port: u16,
    pub(crate) events: &'a EventLog,
}

impl SamplerCtx<'_> {
    /// The cluster, as the supervisor calls address it.
    fn servers(&self) -> crate::agones::Servers<'_> {
        crate::agones::Servers::cluster(self.client, self.namespace, self.control_port)
    }
}

/// Sample every running instance once and append the tick, returning a
/// watchdog observation for every *listed* instance (running, paused, or
/// condemned). Never fails: a listing error is logged and the tick skipped, an
/// unreachable supervisor yields no row (or a null count) but still an
/// observation, and the insert swallows its own errors.
pub(crate) async fn run_occupancy_cycle(ctx: &SamplerCtx<'_>) -> Vec<StatusObservation> {
    let instances = match list_live_instances(ctx.client, ctx.namespace).await {
        Ok(instances) => instances,
        Err(err) => {
            error!(error = ?err, "occupancy sampler could not list servers");
            return Vec::new();
        }
    };

    let mut samples = Vec::new();
    let mut observations = Vec::new();
    for instance in &instances {
        let name = instance.instance.as_str();
        // Agones already condemned it; its pod is gone or going, so there is
        // no supervisor left worth asking.
        if instance.unhealthy {
            observations.push(observation(instance, PollObservation::GameServerUnhealthy));
            continue;
        }
        let status = match supervisor_status(ctx.servers(), ctx.http, name).await {
            Ok(status) => status,
            Err(err) => {
                error!(error = ?err, instance = name, "occupancy sampler: status read failed");
                observations.push(observation(instance, PollObservation::ReadFailed));
                continue;
            }
        };
        observations.push(observation(instance, poll_of(&status)));
        if !instance.running {
            continue;
        }
        let occupancy = match supervisor_occupancy(ctx.servers(), ctx.http, name).await {
            Ok(occupancy) => occupancy,
            Err(err) => {
                error!(error = ?err, instance = name, "occupancy sampler: occupancy read failed");
                continue;
            }
        };
        samples.extend(build_sample(instance, &status, &occupancy));
    }

    // Per-tick, so it can only ever be trace-level noise; the row count is the
    // one thing worth seeing when a sampling gap needs explaining.
    debug!(
        sampled = samples.len(),
        live = instances.len(),
        "occupancy sampler tick"
    );
    ctx.events.record_samples(&samples).await;
    observations
}

/// A status poll's watchdog-facing shape — including the polls that yield no
/// sample row, which are exactly the ones the wedged-supervisor rule feeds on.
fn poll_of(status: &StatusPoll) -> PollObservation {
    match status {
        StatusPoll::Ok(status) => PollObservation::Ok {
            phase: status.process,
            restarts: status.restarts,
            uptime_seconds: status.uptime_seconds,
        },
        StatusPoll::Unreachable => PollObservation::Unreachable,
        StatusPoll::PodNotReady => PollObservation::PodNotReady,
        StatusPoll::NotFound => PollObservation::NotFound,
        StatusPoll::NotManaged => PollObservation::NotManaged,
    }
}

/// One instance's observation this tick, with the identity labels the eventual
/// incident needs to carry.
fn observation(instance: &LiveInstance, poll: PollObservation) -> StatusObservation {
    StatusObservation {
        instance: instance.instance.clone(),
        game: labelled(instance.game.as_str()).map(GameId::new),
        guild: labelled(instance.guild.as_str()).map(GuildId::new),
        poll,
    }
}

/// Fold one instance's two supervisor readings into a row, or into nothing.
///
/// A status that couldn't be read means no row at all: without the restart
/// counter there is no sample, and every reason it can't be read (absent,
/// unmanaged, pod not ready, unreachable) describes an instance mid-transition
/// rather than an idle one. An occupancy that couldn't be read is different —
/// the instance is demonstrably up, so the row is kept with an unknown count.
fn build_sample(
    instance: &LiveInstance,
    status: &StatusPoll,
    occupancy: &FsOutcome<Option<u32>>,
) -> Option<OccupancySample> {
    let StatusPoll::Ok(status) = status else {
        return None;
    };
    let players = match occupancy {
        FsOutcome::Ok(players) => *players,
        FsOutcome::NotFound
        | FsOutcome::NotManaged
        | FsOutcome::PodNotReady
        | FsOutcome::Unreachable
        | FsOutcome::Rejected(_) => None,
    };
    Some(OccupancySample {
        instance: instance.instance.clone(),
        // The lister defaults an absent label to an empty string; an empty
        // string in the column would read as a real (blank) game or guild, so
        // it becomes SQL NULL here instead.
        game: labelled(instance.game.as_str()).map(GameId::new),
        guild: labelled(instance.guild.as_str()).map(GuildId::new),
        players,
        restarts: status.restarts,
    })
}

/// A label value that was actually set, or `None` for the lister's empty-string
/// stand-in for an absent label.
fn labelled(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
#[path = "tests/sampler.rs"]
mod tests;
