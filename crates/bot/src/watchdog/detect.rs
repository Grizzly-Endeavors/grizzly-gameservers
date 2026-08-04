//! Pure incident detection over the sampler's per-tick observations.
//!
//! [`DetectorState::tick`] is a state-transition function: poll results in,
//! fired [`Incident`]s out, no IO. The sampler owns one instance of it and
//! feeds it after each pass, so detection costs nothing the sampler wasn't
//! already paying for.
//!
//! Thresholds are consts, not env knobs, mirroring the defer watcher's timing
//! constants: they encode a judgment ("two unexpected exits in fifteen minutes
//! is a pattern, one is a recovered blip"), and shadow-mode data is the way to
//! revise that judgment before a live deployment leans on it.
//!
//! State is in-memory only. A bot redeploy re-baselines every instance —
//! worst case one repeat wake for a still-broken server, which is the correct
//! side to err on.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use grizzly_control_api::ProcessPhase;
use tracing::debug;

use super::{Incident, IncidentFacts, IncidentKind};
use crate::domain::{GameId, GuildId, InstanceName};
use crate::events::ResourceSample;

/// How many recent ticks of restart deltas the crash-loop rule sums over.
const CRASH_WINDOW_TICKS: u32 = 3;
/// Unexpected exits within the window that make a crash-loop. Two: a single
/// crash-and-relaunch is a recovered blip (and already lands in the
/// crash-frequency series); a second inside the window is a pattern.
const CRASH_RESTARTS_THRESHOLD: u32 = 2;
/// Consecutive unreachable ticks that make a wedged supervisor. Three ticks at
/// the default 5-minute interval is ~15 minutes — past any transient network
/// blip or pod reschedule.
const UNREACHABLE_TICKS: u32 = 3;
/// Whole-pod memory as a percentage of the declared limit that counts as
/// pressure. Conservative in the right direction: actuals include the Agones
/// sidecar while the limit is in practice the game container's alone.
const MEM_PRESSURE_PERCENT: i64 = 90;
/// Consecutive over-threshold ticks that make memory pressure an incident
/// rather than a spike (a world save or a chunk-generation burst).
const MEM_PRESSURE_TICKS: u32 = 3;
/// Per-(instance, kind) refire suppression. An unresolved incident re-wakes
/// Gary once an hour, not every tick.
const INCIDENT_COOLDOWN: Duration = Duration::from_hours(1);

/// What one `/status` poll of one running instance yielded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PollObservation {
    /// The supervisor answered.
    Ok {
        phase: ProcessPhase,
        restarts: u32,
        uptime_seconds: u64,
    },
    /// The pod is up but the control API didn't answer — the wedged signal.
    Unreachable,
    /// The pod exists but isn't ready; mid-transition, not evidence of anything.
    PodNotReady,
    /// The instance vanished between the listing and the poll.
    NotFound,
    /// The pod isn't one the bot manages.
    NotManaged,
    /// The cluster query itself failed; nothing was learned this tick.
    ReadFailed,
}

/// One running instance's status poll this tick, with the identity labels the
/// eventual incident needs to carry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StatusObservation {
    pub(crate) instance: InstanceName,
    pub(crate) game: Option<GameId>,
    pub(crate) guild: Option<GuildId>,
    pub(crate) poll: PollObservation,
}

/// Per-instance detection state.
#[derive(Default)]
struct InstanceWatch {
    game: Option<GameId>,
    guild: Option<GuildId>,
    /// The restart counter at the last successful poll, for differencing.
    last_restarts: Option<u32>,
    /// Positive restart deltas from the most recent ticks, newest last, capped
    /// at [`CRASH_WINDOW_TICKS`].
    deltas: VecDeque<u32>,
    unreachable_streak: u32,
    mem_pressure_streak: u32,
    /// When each incident kind last fired for this instance, for the cooldown.
    last_fired: HashMap<IncidentKind, Instant>,
}

/// The detector: all watched instances' state plus the tick interval (used only
/// to render streak lengths as minutes in incident facts).
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

    /// Fold one sampler tick's observations in and return the incidents that
    /// fired. `now` is injected so cooldown behavior is testable.
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

    /// Forget instances that appeared in neither listing this tick — they were
    /// destroyed (or the world was shut down and its pod released), and keeping
    /// their cooldowns would suppress a fresh incident on a future namesake.
    fn prune(&mut self, statuses: &[StatusObservation], resources: &[ResourceSample]) {
        self.instances.retain(|name, _| {
            statuses.iter().any(|obs| obs.instance == *name)
                || resources.iter().any(|sample| sample.instance == *name)
        });
    }
}

/// Advance one instance's status-side state and fire at most one incident.
fn status_transition(
    watch: &mut InstanceWatch,
    observation: &StatusObservation,
    interval: Duration,
    now: Instant,
) -> Option<Incident> {
    match observation.poll {
        PollObservation::Ok {
            phase,
            restarts,
            uptime_seconds,
        } => {
            watch.unreachable_streak = 0;
            let window_delta = fold_restart_delta(watch, restarts);
            if phase == ProcessPhase::Crashed {
                // The supervisor already applied its own threshold (five exits
                // inside its crash window) before reporting this, so it fires
                // without further judgment here.
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
        // Mid-transition or unlearnable ticks hold every streak: they are not
        // evidence the server is fine, so they must not reset, and not evidence
        // it is stuck, so they must not advance.
        PollObservation::PodNotReady
        | PollObservation::NotFound
        | PollObservation::NotManaged
        | PollObservation::ReadFailed => None,
    }
}

/// Difference the restart counter into the delta ring and return the window
/// sum. A first sighting only baselines; a counter that went *down* means the
/// pod was replaced, which re-baselines silently (the fresh pod's exits start
/// counting from zero).
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

/// Advance one instance's memory-side state and fire at most one incident.
/// A missing reading or a missing limit holds the streak — unknown is not fine
/// and not pressure — and a below-threshold reading resets it.
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

/// Emit an incident unless this (instance, kind) fired inside the cooldown.
/// The cooldown is stamped only on an actual emit, so a suppressed refire
/// doesn't keep pushing the window forward.
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

/// A streak length in wall-clock minutes, for human-readable incident facts.
fn ticks_as_minutes(ticks: u32, interval: Duration) -> u64 {
    u64::from(ticks).saturating_mul(interval.as_secs()) / 60
}

#[cfg(test)]
#[path = "tests/detect.rs"]
mod tests;
