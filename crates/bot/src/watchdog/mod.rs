//! The proactive watchdog: Gary notices a broken server before anyone reports it.
//!
//! Detection rides the occupancy/resource sampler — the one loop that already
//! visits every running server each tick — as pure state transitions over the
//! observations the sampler was discarding ([`detect`]). A fired [`Incident`]
//! crosses an mpsc channel to the responder ([`respond`]), which holds the full
//! command `Data` and reacts according to [`crate::config::WatchdogMode`]:
//! record-only in shadow mode, a manager-tier Gary turn in the guild's home
//! channel when on.
//!
//! The split exists because the sampler is spawned before `Data` is constructed
//! and deliberately owns no Discord reach; the channel is the seam, not a
//! convenience. Detection therefore requires the database (no sampler, no
//! watchdog), which is the deployed reality anyway.

mod detect;
mod respond;

use tokio::sync::mpsc;

pub(crate) use detect::{DetectorState, PollObservation, StatusObservation};
pub(crate) use respond::spawn_responder;

use crate::config::WatchdogMode;
use crate::domain::{GameId, GuildId, InstanceName};

/// How many fired incidents may sit between the sampler and the responder before
/// new ones are dropped. Incidents are rare and cooldown-limited, so a full
/// queue means the responder is wedged — dropping (with a warning) is bounded
/// loss, and a still-broken server re-fires after its cooldown anyway.
const QUEUE_DEPTH: usize = 16;

/// Which problem signal fired. One cooldown key per kind per instance, so the
/// same underlying illness can't wake Gary through two names at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum IncidentKind {
    /// The game process is crashing: repeated unexpected exits inside the
    /// detection window, or the supervisor has already given up relaunching.
    Crash,
    /// The pod claims a running process but the supervisor's control API has
    /// stopped answering.
    Wedged,
    /// Whole-pod memory has been pressing against the declared limit long
    /// enough to be a pattern, not a spike.
    Memory,
}

impl IncidentKind {
    /// The `signal` value in the incident event's details — queried, so a
    /// published vocabulary like the event catalog's own strings.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Crash => "crash",
            Self::Wedged => "wedged",
            Self::Memory => "memory",
        }
    }
}

/// The signal-specific numbers behind a fired incident, carried verbatim into
/// the event log and (when a turn runs) into Gary's incident prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum IncidentFacts {
    /// Unexpected exits accumulated across the detection window while the
    /// supervisor kept relaunching.
    CrashLoop {
        restarts_delta: u32,
        window_minutes: u64,
        total_restarts: u32,
        uptime_seconds: u64,
    },
    /// The supervisor crossed its own crash threshold and stopped relaunching;
    /// the process is down until someone intervenes.
    Crashed { total_restarts: u32 },
    /// Consecutive ticks the control API has been unreachable while the pod
    /// stayed up and labelled running.
    Unreachable { minutes: u64 },
    /// Actual whole-pod memory against the declared limit.
    MemoryPressure {
        used_bytes: i64,
        limit_bytes: i64,
        percent: u8,
    },
}

/// One detected problem on one instance, ready for the responder to act on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Incident {
    pub(crate) instance: InstanceName,
    pub(crate) game: Option<GameId>,
    pub(crate) guild: Option<GuildId>,
    pub(crate) kind: IncidentKind,
    pub(crate) facts: IncidentFacts,
}

/// The sampler→responder incident channel, or `(None, None)` when the watchdog
/// is off — no channel means the sampler skips detection entirely and the
/// responder is never spawned.
pub(crate) fn channel(
    mode: WatchdogMode,
) -> (
    Option<mpsc::Sender<Incident>>,
    Option<mpsc::Receiver<Incident>>,
) {
    match mode {
        WatchdogMode::Off => (None, None),
        WatchdogMode::On | WatchdogMode::Shadow => {
            let (tx, rx) = mpsc::channel(QUEUE_DEPTH);
            (Some(tx), Some(rx))
        }
    }
}
