use kube::Client;
use tracing::{debug, error};

use super::store::EventLog;
use crate::agones::{
    FsOutcome, LiveInstance, StatusPoll, list_live_instances, supervisor_occupancy,
    supervisor_status,
};
use crate::domain::{GameId, GuildId, InstanceName};
use crate::watchdog::{PollObservation, StatusObservation};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OccupancySample {
    pub(crate) instance: InstanceName,
    pub(crate) game: Option<GameId>,
    pub(crate) guild: Option<GuildId>,
    pub(crate) players: Option<u32>,
    pub(crate) restarts: u32,
}

pub(crate) struct SamplerCtx<'a> {
    pub(crate) client: &'a Client,
    pub(crate) http: &'a crate::agones::ControlClient,
    pub(crate) namespace: &'a str,
    pub(crate) control_port: u16,
    pub(crate) events: &'a EventLog,
}

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
        if instance.unhealthy {
            observations.push(observation(instance, PollObservation::GameServerUnhealthy));
            continue;
        }
        let status =
            match supervisor_status(ctx.client, ctx.http, ctx.namespace, name, ctx.control_port)
                .await
            {
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
        let occupancy = match supervisor_occupancy(
            ctx.client,
            ctx.http,
            ctx.namespace,
            name,
            ctx.control_port,
        )
        .await
        {
            Ok(occupancy) => occupancy,
            Err(err) => {
                error!(error = ?err, instance = name, "occupancy sampler: occupancy read failed");
                continue;
            }
        };
        samples.extend(build_sample(instance, &status, &occupancy));
    }

    debug!(
        sampled = samples.len(),
        live = instances.len(),
        "occupancy sampler tick"
    );
    ctx.events.record_samples(&samples).await;
    observations
}

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

fn observation(instance: &LiveInstance, poll: PollObservation) -> StatusObservation {
    StatusObservation {
        instance: instance.instance.clone(),
        game: labelled(instance.game.as_str()).map(GameId::new),
        guild: labelled(instance.guild.as_str()).map(GuildId::new),
        poll,
    }
}

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
        game: labelled(instance.game.as_str()).map(GameId::new),
        guild: labelled(instance.guild.as_str()).map(GuildId::new),
        players,
        restarts: status.restarts,
    })
}

fn labelled(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
#[path = "tests/sampler.rs"]
mod tests;
