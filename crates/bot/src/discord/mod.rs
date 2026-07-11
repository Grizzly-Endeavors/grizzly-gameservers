//! The Discord shell: slash [`commands`], Gary's `@mention` shell ([`gary`]),
//! auth/admin gating ([`auth`]), reply chunking, and embed rendering
//! ([`render`]). Holds [`Data`], the per-command state every handler shares,
//! and defers all cluster/Agones work to `crate::agones`.

mod auth;
mod chunking;
pub(crate) mod commands;
pub(crate) mod gary;
mod render;

pub(crate) use auth::{AccessLevel, require_scope};

use std::sync::Arc;
use std::time::Duration;

use kube::Client;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::agent::{OllamaConfig, SessionStore};
use crate::agones::GameCatalog;
use crate::backup::MaybeBackups;
use crate::defer::DeferRuntime;
use crate::memory::GaryMemory;
use crate::store::{GuildConfig, HomeChannels};

/// How long an interactive component (button, confirm prompt) waits for a
/// friend to respond before the interaction expires. Shared so `/destroy`'s
/// confirm dialog and other component collectors stay in lockstep.
pub(crate) const COMPONENT_TIMEOUT: Duration = Duration::from_mins(2);

/// Per-command state shared with every poise command handler. `Clone` is cheap
/// (handles are `Arc`/client clones) so an event handler can hand an owned copy
/// to a spawned, drainable Gary session (see `crate::run`'s task tracker).
#[derive(Clone)]
pub(crate) struct Data {
    pub(crate) kube_client: Client,
    /// Client for the in-pod supervisor control API (`/stop`, `/start`, `/restart`).
    pub(crate) http: reqwest::Client,
    pub(crate) namespace: String,
    pub(crate) domain: String,
    /// Port the supervisor's control API listens on inside each game pod.
    pub(crate) control_port: u16,
    pub(crate) catalog: Arc<GameCatalog>,
    /// Serializes the port-lease→Service-create critical section across
    /// concurrent `/create`s so two friends can't claim the same `NodePort`.
    pub(crate) provision_lock: Arc<Mutex<()>>,
    /// Cross-guild operator seed (env `GAMESERVERS_ADMIN_USER_IDS`): admin in
    /// every guild and carrying the all-guilds visibility scope.
    pub(crate) operator_ids: Arc<[u64]>,
    /// Per-guild admin config (admin roles/users) set at runtime via `/config`,
    /// backed by Postgres. Empty per guild when persistence is down — auth then
    /// falls back to operators + guild owner (fail-closed).
    pub(crate) guild_config: Arc<GuildConfig>,
    /// Agent ("Gary") model connection, or `None` when no key is configured —
    /// in which case mentions reply that Gary isn't set up.
    pub(crate) ollama: Option<OllamaConfig>,
    /// Short-lived per-`(channel, user)` conversation transcripts giving Gary
    /// follow-up continuity across mentions.
    pub(crate) sessions: Arc<SessionStore>,
    /// Channels where Gary answers without an `@mention` (plus DMs, which are
    /// always no-mention). Backed by Postgres; disabled if persistence is down.
    pub(crate) home_channels: Arc<HomeChannels>,
    /// Durable operational facts Gary has learned per game, injected into his
    /// system prompt. Backed by Postgres; empty and read-only if persistence is
    /// down.
    pub(crate) memory: Arc<GaryMemory>,
    /// S3-backed backups/archive/restore, or `None` when S3 isn't configured (the
    /// backup commands then report "not configured", same shape as Gary/home).
    pub(crate) backup: MaybeBackups,
    /// Tracks spawned Gary sessions so the shutdown drain can await an in-flight
    /// turn (e.g. between a mutating tool call and its follow-up) before exit.
    pub(crate) tasks: TaskTracker,
    /// DMs the operators when Gary escalates a request he couldn't resolve, so the
    /// "flagged for Bear" reply is a promise the system actually keeps.
    pub(crate) notifier: crate::notify::OperatorNotifier,
    /// Gary's deferred-task queue (`run_when`): durable in Valkey, watchers polled
    /// in the background. Disabled (reports it can't schedule) when Valkey isn't
    /// configured, the same graceful-degrade shape as `home_channels`/`backup`.
    pub(crate) defer: Arc<DeferRuntime>,
    /// The shared shutdown signal, so a deferred watcher's long wait can be
    /// cancelled promptly instead of blocking the drain for its full ceiling.
    pub(crate) shutdown: CancellationToken,
}

/// Build the backup-flow context from the shared per-command [`Data`]. Shared by
/// the slash commands and Gary's tools so both drive the backup subsystem the same
/// way.
pub(crate) fn backup_ctx(data: &Data) -> crate::backup::BackupCtx<'_> {
    crate::backup::BackupCtx {
        client: &data.kube_client,
        http: &data.http,
        namespace: &data.namespace,
        domain: &data.domain,
        control_port: data.control_port,
        catalog: &data.catalog,
        provision_lock: &data.provision_lock,
    }
}

pub(crate) type Error = Box<dyn std::error::Error + Send + Sync>;
pub(crate) type Context<'a> = poise::Context<'a, Data, Error>;
