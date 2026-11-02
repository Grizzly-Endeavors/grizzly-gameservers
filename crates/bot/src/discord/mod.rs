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
use poise::serenity_prelude as serenity;
use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{error, info};

use crate::agent::{Environment, OllamaConfig, SessionStore};
use crate::agones::GameCatalog;
use crate::backup::MaybeBackups;
use crate::defer::DeferRuntime;
use crate::memory::GaryMemory;
use crate::sizing::InstanceSizing;
use crate::store::{GuildConfig, HomeChannels};

/// How long an interactive component (button, confirm prompt) waits for a
/// friend to respond before the interaction expires. Shared so `/destroy`'s
/// confirm dialog and other component collectors stay in lockstep.
pub(crate) const COMPONENT_TIMEOUT: Duration = Duration::from_mins(2);

/// The live gateway [`serenity::Context`], published on every `Ready`. The HTTP
/// surface is served before Discord connects, so a route that needs to post a
/// message or collect an interaction has to reach the Context late — and answer
/// honestly when there isn't one yet.
///
/// `watch` rather than a `OnceLock`: `Ready` re-fires on every reconnect, and the
/// freshest Context is the one whose shard messenger is still live. Pinning the
/// first-connect Context would leave a `ComponentInteractionCollector` waiting on
/// a dead shard after any reconnect — a confirmation prompt that silently never
/// resolves, which is the failure mode this project refuses to ship.
#[derive(Clone)]
pub(crate) struct GatewayHandle {
    current: Arc<watch::Sender<Option<serenity::Context>>>,
}

impl GatewayHandle {
    pub(crate) fn new() -> Self {
        Self {
            current: Arc::new(watch::Sender::new(None)),
        }
    }

    /// Publish (or refresh) the live Context. Idempotent, so calling it on every
    /// `Ready` is correct rather than merely tolerated.
    pub(crate) fn publish(&self, ctx: &serenity::Context) {
        self.current.send_replace(Some(ctx.clone()));
    }

    /// The live Context, or `None` before the gateway's first `Ready`. Clones out
    /// of the borrow immediately so no caller can hold the lock across an await.
    pub(crate) fn current(&self) -> Option<serenity::Context> {
        self.current.borrow().clone()
    }

    /// An awaitable view of the live Context, for a background consumer that
    /// would rather wait for the first `Ready` than fail before it — the shape
    /// [`Self::current`]'s honest `None` is wrong for.
    pub(crate) fn subscribe(&self) -> watch::Receiver<Option<serenity::Context>> {
        self.current.subscribe()
    }
}

/// Whether the operator has admitted `guild`. An empty allowlist admits every
/// guild — the posture before the list existed, kept so an un-configured deploy
/// serves the guilds already relying on it rather than going dark.
///
/// Applied to command registration and to every agent surface, because a guild
/// that isn't served must not be able to spend model turns either. Read from the
/// live settings snapshot on every call rather than from a copy, so admitting or
/// revoking a guild in the admin panel binds immediately.
pub(crate) fn guild_is_admitted(data: &Data, guild: u64) -> bool {
    let allowlist = &data.settings.snapshot().guild_allowlist;
    allowlist.is_empty() || allowlist.contains(&guild)
}

/// Per-command state shared with every poise command handler. `Clone` is cheap
/// (handles are `Arc`/client clones) so an event handler can hand an owned copy
/// to a spawned, drainable Gary session (see `crate::run`'s task tracker).
#[derive(Clone)]
pub(crate) struct Data {
    pub(crate) kube_client: Client,
    /// Client for the in-pod supervisor control API (`/stop`, `/start`, `/restart`).
    pub(crate) http: reqwest::Client,
    /// Dials game-pod supervisors, presenting the shared control credential. A
    /// separate client from `http` because that one also reaches Ollama and
    /// Langfuse, and the credential must never leave the cluster.
    pub(crate) control_http: crate::agones::ControlClient,
    pub(crate) namespace: String,
    pub(crate) domain: String,
    /// Port the supervisor's control API listens on inside each game pod.
    pub(crate) control_port: u16,
    /// Where the servers this process operates live: the cluster for the
    /// deployed bot, a fixed set for `gary-local`. Read through [`Self::servers`].
    pub(crate) directory: crate::agones::ServerDirectory,
    pub(crate) catalog: Arc<GameCatalog>,
    /// Serializes the port-lease→Service-create critical section across
    /// concurrent `/create`s so two friends can't claim the same `NodePort`.
    pub(crate) provision_lock: Arc<Mutex<()>>,
    /// Cross-guild operator seed (env `GAMESERVERS_ADMIN_USER_IDS`): admin in
    /// every guild and carrying the all-guilds visibility scope.
    pub(crate) operator_ids: Arc<[u64]>,
    /// The operator-tunable knobs, resolved database override > environment >
    /// compiled default. Read at the point of use — never copied into a field —
    /// so a change made in the admin panel reaches every surface at once.
    pub(crate) settings: Arc<crate::settings::RuntimeSettings>,
    /// Per-guild admin config (admin roles/users) set at runtime via `/config`,
    /// backed by Postgres. Empty per guild when persistence is down — auth then
    /// falls back to operators + guild owner (fail-closed).
    pub(crate) guild_config: Arc<GuildConfig>,
    /// Agent ("Gary") model connection, or `None` when no key is configured —
    /// in which case mentions reply that Gary isn't set up.
    pub(crate) ollama: Option<OllamaConfig>,
    /// Fireworks credential for operator-harness turns that pick that provider.
    /// No Discord or in-game path reads it.
    pub(crate) fireworks_api_key: Option<String>,
    /// Short-lived per-channel conversation transcripts giving Gary follow-up
    /// continuity across messages, shared by everyone talking in that channel.
    pub(crate) sessions: Arc<SessionStore>,
    /// Channels where Gary answers without an `@mention` (plus DMs, which are
    /// always no-mention). Backed by Postgres; disabled if persistence is down.
    pub(crate) home_channels: Arc<HomeChannels>,
    /// Durable operational facts Gary has learned per game, injected into his
    /// system prompt. Backed by Postgres; empty and read-only if persistence is
    /// down.
    pub(crate) memory: Arc<GaryMemory>,
    /// Per-instance CPU/memory overrides, re-applied every time a `GameServer`
    /// is rendered so a resize survives a stop/start. Backed by Postgres; disabled
    /// when persistence is down, which makes a resize a refusal rather than a
    /// change that would evaporate on the next start.
    pub(crate) sizing: Arc<InstanceSizing>,
    /// Per-guild resource caps (ADR-022): global defaults plus operator-set
    /// overrides. Backed by Postgres; with persistence down every guild is
    /// capped at the defaults, so enforcement never turns off.
    pub(crate) guild_limits: Arc<crate::limits::GuildLimits>,
    /// S3-backed backups/archive/restore, or `None` when S3 isn't configured (the
    /// backup commands then report "not configured", same shape as Gary/home).
    pub(crate) backup: MaybeBackups,
    /// Admission control for model-driven turns: a per-subject rate bucket and a
    /// global concurrency bound. Every surface that can start a paid turn — a
    /// Discord mention, a home-channel message, an in-game `@Gary` — passes
    /// through it, because each is reachable by someone the operator never vetted.
    pub(crate) turn_throttle: std::sync::Arc<crate::throttle::TurnThrottle>,
    /// The Gary turns currently running, so `/gary-stop` can end one.
    pub(crate) interrupts: std::sync::Arc<crate::interrupt::TurnInterrupts>,
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
    /// The live gateway Context, for the HTTP routes that are served before
    /// Discord connects. Holds a Context and never `Data`, so there's no cycle.
    pub(crate) gateway: GatewayHandle,
    /// Which population this process's ordinary traffic belongs to, stamped on
    /// every turn's identity.
    pub(crate) environment: Environment,
    /// Exports every model-driven turn to Langfuse. Disabled (and silently
    /// discarding) when the key pair isn't configured.
    pub(crate) telemetry: Arc<crate::telemetry::Telemetry>,
    /// The permanent product record: who created, started, and destroyed what,
    /// from which surface. Backed by Postgres; a disabled log accepts and
    /// discards, so emission sites never branch on it.
    pub(crate) events: Arc<crate::events::EventLog>,
    /// The beta signup queue: fed by the public landing page through the intake
    /// route, worked through in the admin panel. Disabled without a database,
    /// which the panel renders as a notice rather than an error.
    pub(crate) signups: Arc<crate::signups::Signups>,
    /// Sends the beta-approval email. Disabled without an SMTP password, in
    /// which case an approval is still recorded and can be mailed later.
    pub(crate) mailer: Arc<crate::mail::Mailer>,
}

impl Data {
    /// Gary's model connection as it stands right now, or `None` when no API key
    /// was configured at startup.
    ///
    /// The credential half is frozen at boot — it is a secret, so it arrives
    /// through ESO and rotating it is a redeploy — while the model tag and base
    /// URL come from the live settings. **Call this once, at the start of a
    /// turn, and use the result for every round**: a turn that re-read the model
    /// between rounds could switch models mid-conversation, which reads to the
    /// person as Gary changing his mind about what he is.
    pub(crate) fn ollama_now(&self) -> Option<OllamaConfig> {
        ollama_now(self.ollama.as_ref(), &self.settings)
    }

    /// Where this process finds a server's supervisor, for every control call.
    pub(crate) fn servers(&self) -> crate::agones::Servers<'_> {
        match &self.directory {
            crate::agones::ServerDirectory::Cluster => crate::agones::Servers::cluster(
                &self.kube_client,
                &self.namespace,
                self.control_port,
            ),
            crate::agones::ServerDirectory::Fixed(fixed) => crate::agones::Servers::Fixed(fixed),
        }
    }
}

/// Merge the boot credential with the live model settings. Free of [`Data`] so
/// the in-game endpoint, which carries its own carved-out handles, resolves the
/// connection exactly the same way.
pub(crate) fn ollama_now(
    boot: Option<&OllamaConfig>,
    settings: &crate::settings::RuntimeSettings,
) -> Option<OllamaConfig> {
    let boot = boot?;
    let effective = settings.snapshot();
    Some(OllamaConfig {
        api_key: boot.api_key.clone(),
        base_url: effective.gary_base_url.clone(),
        model: effective.gary_model.clone(),
        max_tokens: None,
    })
}

/// The slash commands the bot registers with each guild it's in.
pub(crate) fn slash_commands() -> Vec<poise::Command<Data, Error>> {
    vec![
        self::commands::servers(),
        self::commands::create(),
        self::commands::shutdown(),
        self::commands::stop(),
        self::commands::start(),
        self::commands::restart(),
        self::commands::destroy(),
        self::commands::backup(),
        self::commands::backups(),
        self::commands::archive(),
        self::commands::archives(),
        self::commands::delete_archive(),
        self::commands::restore(),
        self::commands::recover(),
        self::commands::new_session(),
        self::commands::gary_stop(),
        self::commands::gary_home(),
        self::commands::gary_memory(),
        self::commands::config(),
    ]
}

/// Register (or revoke) this guild's slash commands.
///
/// An unadmitted guild is registered with an *empty* set rather than skipped: a
/// guild that has just been removed from the allowlist must lose the commands it
/// already has, and re-registering empty is how that revocation reaches Discord.
/// Idempotent, so the `GuildCreate` replays on every gateway reconnect are free.
///
/// # Errors
///
/// Propagates a serenity failure from the registration call.
pub(crate) async fn register_guild_commands(
    ctx: &serenity::Context,
    commands: &[poise::Command<Data, Error>],
    guild: serenity::GuildId,
    admitted: bool,
) -> Result<(), Error> {
    let revoked: Vec<poise::Command<Data, Error>> = Vec::new();
    let registering = if admitted { commands } else { &revoked };
    poise::builtins::register_in_guild(ctx, registering, guild).await?;
    Ok(())
}

/// What a registration sweep did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SweepOutcome {
    pub(crate) served: usize,
    pub(crate) revoked: usize,
    pub(crate) failed: usize,
}

/// Re-register every guild the bot is in against the current allowlist.
///
/// The allowlist is read live, so admission binds immediately for Gary and for
/// the command *check*. Discord's own command list, though, is a thing the bot
/// pushed at some point — changing the allowlist doesn't retract it. This is what
/// does, and it is why an admission change is a push rather than a pure pull.
///
/// Returns `None` when the gateway isn't connected: there is nothing to register
/// against yet, and the `GuildCreate` replay on reconnect reconciles anyway.
pub(crate) async fn registration_sweep(data: &Data) -> Option<SweepOutcome> {
    let ctx = data.gateway.current()?;
    let commands = slash_commands();
    let mut outcome = SweepOutcome::default();
    for guild in ctx.cache.guilds() {
        let admitted = guild_is_admitted(data, guild.get());
        match register_guild_commands(&ctx, &commands, guild, admitted).await {
            Ok(()) if admitted => outcome.served += 1,
            Ok(()) => outcome.revoked += 1,
            Err(err) => {
                error!(
                    error = ?err,
                    guild = guild.get(),
                    "failed to re-register commands for a guild after an admission change"
                );
                outcome.failed += 1;
            }
        }
    }
    info!(
        served = outcome.served,
        revoked = outcome.revoked,
        failed = outcome.failed,
        "re-registered slash commands after an admission change"
    );
    Some(outcome)
}

/// Push a settings change to the consumers that can't read the snapshot at the
/// point of use.
///
/// Almost every consumer pulls, which is what makes a hot setting reliably hot.
/// Two can't: the turn throttle holds a live semaphore rather than a number, and
/// Discord's registered command list is remote state that only a push can
/// correct. Routing both through one function is what stops a future mutation
/// site from remembering one and forgetting the other.
pub(crate) async fn settings_changed(
    data: &Data,
    key: crate::settings::SettingKey,
    snapshot: &crate::settings::SettingsSnapshot,
) {
    data.turn_throttle.update_policy(snapshot.throttle);
    if key == crate::settings::SettingKey::GuildAllowlist {
        registration_sweep(data).await;
    }
}

/// Build the backup-flow context from the shared per-command [`Data`]. Shared by
/// the slash commands and Gary's tools so both drive the backup subsystem the same
/// way.
pub(crate) fn backup_ctx(data: &Data) -> crate::backup::BackupCtx<'_> {
    crate::backup::BackupCtx {
        client: &data.kube_client,
        http: &data.control_http,
        namespace: &data.namespace,
        domain: &data.domain,
        control_port: data.control_port,
        catalog: &data.catalog,
        provision_lock: &data.provision_lock,
        sizing: &data.sizing,
        guild_limits: &data.guild_limits,
    }
}

/// Whether cold-starting `instance` would push its owning guild over its
/// CPU/memory allowance (ADR-022) — `None` means it may proceed. Shared by the
/// `/start` cold branch and Gary's cold-start tool so the two shells can't
/// drift; call it (and the `begin_start` that follows) under one hold of
/// `Data::provision_lock` so a concurrent create can't add load in between.
///
/// Only the *pod-recreating* start is gated. A warm resume never dropped its
/// pod, so its load never left the aggregate; the backup cycle's ensure-pod and
/// the stranded reconciler restore previously-admitted load and are exempt by
/// construction (they call `begin_start` directly).
///
/// The added load is the instance's recorded override, else its catalog sizing;
/// an unreadable catalog contributes nothing, the same permissiveness as create.
///
/// # Errors
///
/// Returns an error if the owning Service or the guild's live usage can't be
/// read from the Kubernetes API.
pub(crate) async fn cold_start_refusal(
    data: &Data,
    instance: &str,
) -> anyhow::Result<Option<crate::limits::LimitRefusal>> {
    let Some(owner) = data.servers().owner_of(instance).await? else {
        return Ok(None);
    };
    let Some(guild) = owner.guild else {
        // Pre-scoping or operator-created: no tenant, no caps.
        return Ok(None);
    };
    let adding = match data.sizing.get(instance).await {
        Some(sizing) => Some(sizing),
        None => owner
            .game
            .and_then(|game| data.catalog.get(game.as_str()))
            .and_then(crate::agones::catalog_sizing),
    };
    let Some(adding) = adding else {
        return Ok(None);
    };
    let caps = data.guild_limits.caps_for(guild.as_str()).await;
    let usage = crate::agones::guild_usage(
        &data.kube_client,
        &data.namespace,
        guild.as_str(),
        Some(instance),
    )
    .await?;
    Ok(crate::limits::check_added_load(&usage, adding, &caps).err())
}

pub(crate) type Error = Box<dyn std::error::Error + Send + Sync>;
pub(crate) type Context<'a> = poise::Context<'a, Data, Error>;
