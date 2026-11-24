mod detect;
mod respond;

use tokio::sync::mpsc;

pub(crate) use detect::{DetectorState, PollObservation, StatusObservation};
pub(crate) use respond::spawn_responder;

use crate::domain::{GameId, GuildId, InstanceName};

const QUEUE_DEPTH: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum IncidentKind {
    Crash,
    Wedged,
    Memory,
}

impl IncidentKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Crash => "crash",
            Self::Wedged => "wedged",
            Self::Memory => "memory",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum IncidentFacts {
    CrashLoop {
        restarts_delta: u32,
        window_minutes: u64,
        total_restarts: u32,
        uptime_seconds: u64,
    },
    Crashed {
        total_restarts: u32,
    },
    Unreachable {
        minutes: u64,
    },
    Unhealthy,
    MemoryPressure {
        used_bytes: i64,
        limit_bytes: i64,
        percent: u8,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Incident {
    pub(crate) instance: InstanceName,
    pub(crate) game: Option<GameId>,
    pub(crate) guild: Option<GuildId>,
    pub(crate) kind: IncidentKind,
    pub(crate) facts: IncidentFacts,
}

pub(crate) fn channel() -> (mpsc::Sender<Incident>, mpsc::Receiver<Incident>) {
    mpsc::channel(QUEUE_DEPTH)
}
