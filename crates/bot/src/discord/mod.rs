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

pub(crate) const COMPONENT_TIMEOUT: Duration = Duration::from_mins(2);

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

    pub(crate) fn publish(&self, ctx: &serenity::Context) {
        self.current.send_replace(Some(ctx.clone()));
    }

    pub(crate) fn current(&self) -> Option<serenity::Context> {
        self.current.borrow().clone()
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<Option<serenity::Context>> {
        self.current.subscribe()
    }
}

pub(crate) fn guild_is_admitted(data: &Data, guild: u64) -> bool {
    let allowlist = &data.settings.snapshot().guild_allowlist;
    allowlist.is_empty() || allowlist.contains(&guild)
}

#[derive(Clone)]
pub(crate) struct Data {
    pub(crate) kube_client: Client,
    pub(crate) http: reqwest::Client,
    pub(crate) control_http: crate::agones::ControlClient,
    pub(crate) namespace: String,
    pub(crate) domain: String,
    pub(crate) control_port: u16,
    pub(crate) catalog: Arc<GameCatalog>,
    pub(crate) provision_lock: Arc<Mutex<()>>,
    pub(crate) operator_ids: Arc<[u64]>,
    pub(crate) settings: Arc<crate::settings::RuntimeSettings>,
    pub(crate) guild_config: Arc<GuildConfig>,
    pub(crate) ollama: Option<OllamaConfig>,
    pub(crate) sessions: Arc<SessionStore>,
    pub(crate) home_channels: Arc<HomeChannels>,
    pub(crate) memory: Arc<GaryMemory>,
    pub(crate) sizing: Arc<InstanceSizing>,
    pub(crate) guild_limits: Arc<crate::limits::GuildLimits>,
    pub(crate) backup: MaybeBackups,
    pub(crate) turn_throttle: std::sync::Arc<crate::throttle::TurnThrottle>,
    pub(crate) interrupts: std::sync::Arc<crate::interrupt::TurnInterrupts>,
    pub(crate) tasks: TaskTracker,
    pub(crate) notifier: crate::notify::OperatorNotifier,
    pub(crate) defer: Arc<DeferRuntime>,
    pub(crate) shutdown: CancellationToken,
    pub(crate) gateway: GatewayHandle,
    pub(crate) environment: Environment,
    pub(crate) telemetry: Arc<crate::telemetry::Telemetry>,
    pub(crate) events: Arc<crate::events::EventLog>,
    pub(crate) signups: Arc<crate::signups::Signups>,
    pub(crate) mailer: Arc<crate::mail::Mailer>,
}

impl Data {
    pub(crate) fn ollama_now(&self) -> Option<OllamaConfig> {
        ollama_now(self.ollama.as_ref(), &self.settings)
    }
}

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
    })
}

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

pub(crate) async fn cold_start_refusal(
    data: &Data,
    instance: &str,
) -> anyhow::Result<Option<crate::limits::LimitRefusal>> {
    let Some(owner) = crate::agones::owner_of(&data.kube_client, &data.namespace, instance).await?
    else {
        return Ok(None);
    };
    let Some(guild) = owner.guild else {
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
