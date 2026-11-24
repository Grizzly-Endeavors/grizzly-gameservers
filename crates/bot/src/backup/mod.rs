mod manifest;
mod orchestrate;
mod s3;
mod store;

use std::sync::Arc;
use std::time::Duration;

use kube::Client;
use tokio::sync::Mutex;

use crate::agones::GameCatalog;
use s3::S3Store;
use store::ArchiveStore;

pub(crate) struct BackupService {
    s3: S3Store,
    archives: ArchiveStore,
    archive_http: crate::agones::ControlClient,
    retention: usize,
    archive_retention_days: i32,
    interval: Duration,
    events: std::sync::Arc<crate::events::EventLog>,
}

pub(crate) struct BackupCtx<'a> {
    pub(crate) client: &'a Client,
    pub(crate) http: &'a crate::agones::ControlClient,
    pub(crate) namespace: &'a str,
    pub(crate) domain: &'a str,
    pub(crate) control_port: u16,
    pub(crate) catalog: &'a GameCatalog,
    pub(crate) provision_lock: &'a Mutex<()>,
    pub(crate) sizing: &'a crate::sizing::InstanceSizing,
    pub(crate) guild_limits: &'a crate::limits::GuildLimits,
}

#[derive(Debug)]
pub(crate) enum BackupOutcome {
    BackedUp { size_bytes: u64 },
    NotFound,
    NotManaged,
    NotRunning,
    Unreachable(String),
}

impl BackupOutcome {
    pub(crate) fn reason(&self) -> Option<&str> {
        match self {
            Self::Unreachable(reason) => Some(reason),
            Self::BackedUp { .. } | Self::NotFound | Self::NotManaged | Self::NotRunning => None,
        }
    }
}

pub(crate) enum ArchiveOutcome {
    Archived {
        name: String,
        size_bytes: u64,
    },
    ArchivedNotReleased {
        name: String,
        size_bytes: u64,
    },
    NotFound,
    NotManaged,
    /// The archive catalog (Postgres) isn't configured, so archive is disabled.
    Unavailable,
    Failed(String),
}

pub(crate) enum ArchiveDeleteOutcome {
    Deleted {
        name: String,
        generations: usize,
        freed_bytes: u64,
    },
    NotFound,
    /// The archive catalog (Postgres) isn't configured, so nothing can be deleted.
    Unavailable,
    Failed(String),
}

impl ArchiveDeleteOutcome {
    pub(crate) fn reason(&self) -> Option<&str> {
        match self {
            Self::Failed(reason) => Some(reason),
            Self::Deleted { .. } | Self::NotFound | Self::Unavailable => None,
        }
    }
}

impl ArchiveOutcome {
    pub(crate) fn reason(&self) -> Option<&str> {
        match self {
            Self::Failed(reason) => Some(reason),
            Self::Archived { .. }
            | Self::ArchivedNotReleased { .. }
            | Self::NotFound
            | Self::NotManaged
            | Self::Unavailable => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum BootState {
    Ready,
    Crashed,
    Stopped,
    TimedOut,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum SafetyBackup {
    Taken,
    Absent,
}

pub(crate) enum RestoreOutcome {
    Restored {
        boot: BootState,
        safety_backup: SafetyBackup,
    },
    NotFound,
    NotManaged,
    Failed(String),
}

impl RestoreOutcome {
    pub(crate) fn reason(&self) -> Option<&str> {
        match self {
            Self::Failed(reason) => Some(reason),
            Self::Restored { .. } | Self::NotFound | Self::NotManaged => None,
        }
    }
}

pub(crate) enum RecoverOutcome {
    Recovered { address: String, boot: BootState },
    NoSuchArchive,
    NameInUse,
    Unavailable,
    UnknownGame(String),
    PortsExhausted,
    GuildLimitReached(crate::limits::LimitRefusal),
    ClusterFull(crate::capacity::CapacityRefusal),
    Failed(String),
}

impl RecoverOutcome {
    pub(crate) fn reason(&self) -> Option<&str> {
        match self {
            Self::Failed(reason) => Some(reason),
            Self::Recovered { .. }
            | Self::NoSuchArchive
            | Self::NameInUse
            | Self::Unavailable
            | Self::UnknownGame(_)
            | Self::PortsExhausted
            | Self::GuildLimitReached(_)
            | Self::ClusterFull(_) => None,
        }
    }
}

pub(crate) struct ArtifactSummary {
    pub(crate) name: String,
    pub(crate) guild: String,
    pub(crate) key: String,
    pub(crate) size_bytes: u64,
    pub(crate) created_at: String,
}

pub(crate) type MaybeBackups = Option<Arc<BackupService>>;
