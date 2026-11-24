use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use grizzly_control_api::ProcessPhase;
use tracing::debug;

use super::{Incident, IncidentFacts, IncidentKind};
use crate::domain::{GameId, GuildId, InstanceName};
use crate::events::ResourceSample;

const CRASH_WINDOW_TICKS: u32 = 3;
const CRASH_RESTARTS_THRESHOLD: u32 = 2;
const UNREACHABLE_TICKS: u32 = 3;
const MEM_PRESSURE_PERCENT: i64 = 90;
const MEM_PRESSURE_TICKS: u32 = 3;
const INCIDENT_COOLDOWN: Duration = Duration::from_hours(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PollObservation {
    Ok {
        phase: ProcessPhase,
        restarts: u32,
        uptime_seconds: u64,
    },
    Unreachable,
    GameServerUnhealthy,
    PodNotReady,
    NotFound,
    NotManaged,
    ReadFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StatusObservation {
    pub(crate) instance: InstanceName,
    pub(crate) game: Option<GameId>,
    pub(crate) guild: Option<GuildId>,
    pub(crate) poll: PollObservation,
}

#[derive(Default)]
struct InstanceWatch {
    game: Option<GameId>,
    guild: Option<GuildId>,
    last_restarts: Option<u32>,
    deltas: VecDeque<u32>,
    unreachable_streak: u32,
    mem_pressure_streak: u32,
    unhealthy_latched: bool,
    last_fired: HashMap<IncidentKind, Instant>,
}

pub(crate) struct DetectorState {
    interval: Duration,
    instances: HashMap<InstanceName, InstanceWatch>,
}

impl DetectorState {
    pub(crate) fn new(interval: Duration) -> Self {
        Self {
            interval,
            instances: HashMap::new(),
        }
    }

    pub(crate) fn tick(
        &mut self,
        now: Instant,
        statuses: &[StatusObservation],
        resources: &[ResourceSample],
    ) -> Vec<Incident> {
        let mut fired = Vec::new();
        for observation in statuses {
            let watch = self
                .instances
                .entry(observation.instance.clone())
                .or_default();
            watch.game.clone_from(&observation.game);
            watch.guild.clone_from(&observation.guild);
            if let Some(incident) = status_transition(watch, observation, self.interval, now) {
                fired.push(incident);
            }
        }
        for sample in resources {
            let watch = self.instances.entry(sample.instance.clone()).or_default();
            if watch.game.is_none() {
                watch.game.clone_from(&sample.game);
            }
            if watch.guild.is_none() {
                watch.guild.clone_from(&sample.guild);
            }
            if let Some(incident) = memory_transition(watch, sample, now) {
                fired.push(incident);
            }
        }
        self.prune(statuses, resources);
        fired
    }

    fn prune(&mut self, statuses: &[StatusObservation], resources: &[ResourceSample]) {
        self.instances.retain(|name, _| {
            statuses.iter().any(|obs| obs.instance == *name)
                || resources.iter().any(|sample| sample.instance == *name)
        });
    }
}

fn status_transition(
    watch: &mut InstanceWatch,
    observation: &StatusObservation,
    interval: Duration,
    now: Instant,
) -> Option<Incident> {
    if observation.poll != PollObservation::GameServerUnhealthy {
        watch.unhealthy_latched = false;
    }
    match observation.poll {
        PollObservation::GameServerUnhealthy => {
            if watch.unhealthy_latched {
                return None;
            }
            watch.unhealthy_latched = true;
            fire(
                watch,
                &observation.instance,
                IncidentKind::Crash,
                IncidentFacts::Unhealthy,
                now,
            )
        }
        PollObservation::Ok {
            phase,
            restarts,
            uptime_seconds,
        } => {
            watch.unreachable_streak = 0;
            let window_delta = fold_restart_delta(watch, restarts);
            if phase == ProcessPhase::Crashed {
                return fire(
                    watch,
                    &observation.instance,
                    IncidentKind::Crash,
                    IncidentFacts::Crashed {
                        total_restarts: restarts,
                    },
                    now,
                );
            }
            if window_delta >= CRASH_RESTARTS_THRESHOLD {
                return fire(
                    watch,
                    &observation.instance,
                    IncidentKind::Crash,
                    IncidentFacts::CrashLoop {
                        restarts_delta: window_delta,
                        window_minutes: ticks_as_minutes(CRASH_WINDOW_TICKS, interval),
                        total_restarts: restarts,
                        uptime_seconds,
                    },
                    now,
                );
            }
            None
        }
        PollObservation::Unreachable => {
            watch.unreachable_streak = watch.unreachable_streak.saturating_add(1);
            if watch.unreachable_streak >= UNREACHABLE_TICKS {
                return fire(
                    watch,
                    &observation.instance,
                    IncidentKind::Wedged,
                    IncidentFacts::Unreachable {
                        minutes: ticks_as_minutes(watch.unreachable_streak, interval),
                    },
                    now,
                );
            }
            None
        }
        PollObservation::PodNotReady
        | PollObservation::NotFound
        | PollObservation::NotManaged
        | PollObservation::ReadFailed => None,
    }
}

fn fold_restart_delta(watch: &mut InstanceWatch, restarts: u32) -> u32 {
    let window = usize::try_from(CRASH_WINDOW_TICKS).unwrap_or(usize::MAX);
    match watch.last_restarts {
        Some(last) if restarts >= last => {
            watch.deltas.push_back(restarts - last);
            while watch.deltas.len() > window {
                watch.deltas.pop_front();
            }
        }
        Some(_) => watch.deltas.clear(),
        None => {}
    }
    watch.last_restarts = Some(restarts);
    watch
        .deltas
        .iter()
        .fold(0_u32, |sum, delta| sum.saturating_add(*delta))
}

fn memory_transition(
    watch: &mut InstanceWatch,
    sample: &ResourceSample,
    now: Instant,
) -> Option<Incident> {
    let (Some(used), Some(limit)) = (sample.mem_bytes, sample.mem_limit_bytes) else {
        return None;
    };
    if limit <= 0 {
        return None;
    }
    let over = used.saturating_mul(100) >= limit.saturating_mul(MEM_PRESSURE_PERCENT);
    if !over {
        watch.mem_pressure_streak = 0;
        return None;
    }
    watch.mem_pressure_streak = watch.mem_pressure_streak.saturating_add(1);
    if watch.mem_pressure_streak < MEM_PRESSURE_TICKS {
        return None;
    }
    let percent =
        u8::try_from(used.saturating_mul(100).checked_div(limit).unwrap_or(0)).unwrap_or(u8::MAX);
    let facts = IncidentFacts::MemoryPressure {
        used_bytes: used,
        limit_bytes: limit,
        percent,
    };
    fire(watch, &sample.instance, IncidentKind::Memory, facts, now)
}

fn fire(
    watch: &mut InstanceWatch,
    instance: &InstanceName,
    kind: IncidentKind,
    facts: IncidentFacts,
    now: Instant,
) -> Option<Incident> {
    if let Some(last) = watch.last_fired.get(&kind)
        && now.duration_since(*last) < INCIDENT_COOLDOWN
    {
        debug!(
            instance = %instance,
            signal = kind.as_str(),
            "watchdog signal still firing inside its cooldown; suppressed"
        );
        return None;
    }
    watch.last_fired.insert(kind, now);
    Some(Incident {
        instance: instance.clone(),
        game: watch.game.clone(),
        guild: watch.guild.clone(),
        kind,
        facts,
    })
}

fn ticks_as_minutes(ticks: u32, interval: Duration) -> u64 {
    u64::from(ticks).saturating_mul(interval.as_secs()) / 60
}

#[cfg(test)]
#[path = "tests/detect.rs"]
mod tests;
