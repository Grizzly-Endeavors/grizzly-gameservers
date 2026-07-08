//! S3-backed backups, archive, and restore for per-instance world data.
//!
//! Three capabilities, all bot-orchestrated (the bot streams a tar of `/data`
//! from the supervisor's control API straight to S3, and back for restore — S3
//! credentials never leave the bot):
//!
//! - **Automatic/manual backups** — periodic consistent snapshots of a *live*
//!   server to `backups/<instance>/`, for point-in-time restore. The instance is
//!   their index (a prefix listing), so they need no database.
//! - **Archive** — stop a server, back it up to `archives/<channel>/<name>/`, then
//!   release the whole trio (PVC included). Indexed in Postgres because the
//!   instance is gone; the S3 manifest sidecar remains the durable source of truth.
//! - **Restore** — roll a live server back to one of its backups, or recover an
//!   archived one (recreate the trio, reseed `/data` before first launch).
//!
//! Layout, key scheme, and retention live in [`manifest`]; the S3 wire shell in
//! [`s3`]; the archive index in [`store`]; and the flows in [`orchestrate`].

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

/// The backups subsystem: the S3 wire shell, the archive index, and the
/// long-timeout client used to stream a whole world to/from the supervisor and
/// S3. Present in [`crate::discord::Data`] only when S3 is configured.
pub(crate) struct BackupService {
    s3: S3Store,
    archives: ArchiveStore,
    /// Long-timeout client for the streaming archive routes and S3 — distinct
    /// from the bot's short-timeout control client, which is tuned for the quick
    /// lifecycle calls.
    stream_http: reqwest::Client,
    /// Backups kept per server before the scheduled cycle prunes older ones.
    retention: usize,
    /// How often the scheduled cycle runs (used by the timer in `crate::run`).
    interval: Duration,
}

/// The cluster/catalog handles a backup flow needs, bundled so the flow signatures
/// stay within the argument budget. Built from [`crate::discord::Data`] (or the
/// equivalent in `crate::run` for the scheduled cycle) at each call site.
pub(crate) struct BackupCtx<'a> {
    pub(crate) client: &'a Client,
    /// Short-timeout control client for the quick lifecycle calls (stop/start/wait).
    pub(crate) http: &'a reqwest::Client,
    pub(crate) namespace: &'a str,
    pub(crate) domain: &'a str,
    pub(crate) control_port: u16,
    pub(crate) catalog: &'a GameCatalog,
    pub(crate) provision_lock: &'a Mutex<()>,
}

/// Outcome of a manual or scheduled backup of a live server.
pub(crate) enum BackupOutcome {
    BackedUp {
        size_bytes: u64,
    },
    /// No server by that name.
    NotFound,
    /// The server exists but isn't shim-managed.
    NotManaged,
    /// The server has no live pod to snapshot (shut down) — start it first.
    NotRunning,
    /// The supervisor couldn't be reached or the snapshot failed; carries a
    /// developer-facing reason.
    Unreachable(String),
}

impl BackupOutcome {
    /// The developer-facing failure reason to log, if this outcome carries one.
    /// The user-facing embed stays plain-language; this is for the operator's log.
    pub(crate) fn reason(&self) -> Option<&str> {
        match self {
            Self::Unreachable(reason) => Some(reason),
            Self::BackedUp { .. } | Self::NotFound | Self::NotManaged | Self::NotRunning => None,
        }
    }
}

/// Outcome of archiving a server (back up, then release the PVC).
pub(crate) enum ArchiveOutcome {
    Archived {
        name: String,
        size_bytes: u64,
    },
    NotFound,
    NotManaged,
    /// The archive catalog (Postgres) isn't configured, so archive is disabled.
    Unavailable,
    /// A step couldn't be completed; carries a developer-facing reason.
    Failed(String),
}

impl ArchiveOutcome {
    /// The developer-facing failure reason to log, if any.
    pub(crate) fn reason(&self) -> Option<&str> {
        match self {
            Self::Failed(reason) => Some(reason),
            Self::Archived { .. } | Self::NotFound | Self::NotManaged | Self::Unavailable => None,
        }
    }
}

/// Outcome of restoring a live server from one of its backups.
pub(crate) enum RestoreOutcome {
    Restored { ready: bool },
    NotFound,
    NotManaged,
    Failed(String),
}

impl RestoreOutcome {
    /// The developer-facing failure reason to log, if any.
    pub(crate) fn reason(&self) -> Option<&str> {
        match self {
            Self::Failed(reason) => Some(reason),
            Self::Restored { .. } | Self::NotFound | Self::NotManaged => None,
        }
    }
}

/// Outcome of recovering an archived server (recreate the trio, reseed `/data`).
pub(crate) enum RecoverOutcome {
    Recovered {
        address: String,
        ready: bool,
    },
    /// No archive by that name in this channel.
    NoSuchArchive,
    /// A live server already holds that name.
    NameInUse,
    /// The archive catalog isn't configured.
    Unavailable,
    /// The archived game is no longer in the catalog.
    UnknownGame(String),
    /// No free port to recreate the server.
    PortsExhausted,
    Failed(String),
}

impl RecoverOutcome {
    /// The developer-facing failure reason to log, if any.
    pub(crate) fn reason(&self) -> Option<&str> {
        match self {
            Self::Failed(reason) => Some(reason),
            Self::Recovered { .. }
            | Self::NoSuchArchive
            | Self::NameInUse
            | Self::Unavailable
            | Self::UnknownGame(_)
            | Self::PortsExhausted => None,
        }
    }
}

/// One backup or archive shown to a friend in a listing.
pub(crate) struct ArtifactSummary {
    /// Server (instance) name.
    pub(crate) name: String,
    /// The S3 tarball key — the stable handle the restore/recover picks by.
    pub(crate) key: String,
    /// Compressed size in bytes.
    pub(crate) size_bytes: u64,
    /// RFC-3339-ish creation timestamp for display.
    pub(crate) created_at: String,
}

/// Shared handle to the backups subsystem, or `None` when S3 isn't configured.
pub(crate) type MaybeBackups = Option<Arc<BackupService>>;
