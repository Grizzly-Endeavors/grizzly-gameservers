//! `grizzly-gameservers`: the Discord shim, ops agent, and Agones client for
//! friends to spin up and manage game servers. [`discord`] owns the slash
//! commands and Gary's Discord-facing shell; [`agent`] is Gary's reusable
//! chat-completions/tool-calling core; [`agones`] talks to Kubernetes and
//! Agones. [`run`] wires them together and drives the gateway loop.

mod agent;
mod agones;
mod backup;
mod config;
mod discord;
mod domain;
mod ingame;
mod memory;
mod notify;
mod store;

pub use config::BotConfig;

use anyhow::{Context as _, Result};
use poise::serenity_prelude as serenity;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{debug, error, info, warn};

use agent::{OllamaConfig, SessionStore};
use discord::{Data, commands};
use memory::GaryMemory;
use store::{GuildConfig, HomeChannels};

/// Default timeout for supervisor control-API requests. The API is one in-cluster
/// hop away, so a slow response usually means a stuck pod, not a far server. The
/// mutating stop/restart calls override this per-request (they block on the in-pod
/// graceful stop) — see `CONTROL_MUTATION_TIMEOUT` in `agones::supervisor`.
const SUPERVISOR_HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// How long, after the gateway stops, to let in-flight work (Gary sessions, a
/// running backup cycle, in-game answers) finish before the process exits anyway.
/// Kubernetes' default `terminationGracePeriodSeconds` is 30s, so stay under it.
const SHUTDOWN_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Start the Discord bot: connect to Kubernetes, register slash commands with
/// each guild it's in, and run the gateway loop until a shutdown signal arrives.
///
/// # Errors
///
/// Returns an error if the Kubernetes client cannot be initialized, the Discord
/// client cannot be built, or the gateway loop terminates abnormally.
pub async fn run(config: BotConfig) -> Result<()> {
    let kube_client = kube::Client::try_default()
        .await
        .context("failed to initialize kubernetes client")?;

    let catalog = std::sync::Arc::new(
        agones::load_catalog(&config.catalog_dir)
            .await
            .with_context(|| {
                format!(
                    "failed to load game catalog from {}",
                    config.catalog_dir.display()
                )
            })?,
    );

    let http = reqwest::Client::builder()
        .timeout(SUPERVISOR_HTTP_TIMEOUT)
        .build()
        .context("failed to build supervisor control http client")?;

    let namespace = config.namespace;
    let domain = config.domain;
    let control_port = config.control_port;
    let operator_ids: std::sync::Arc<[u64]> = config.operator_ids.into();
    let provision_lock = std::sync::Arc::new(tokio::sync::Mutex::new(()));
    let sessions = std::sync::Arc::new(SessionStore::new());
    let home_channels = std::sync::Arc::new(HomeChannels::connect(config.db.as_ref()).await);
    let guild_config = std::sync::Arc::new(GuildConfig::connect(config.db.as_ref()).await);
    let memory = std::sync::Arc::new(GaryMemory::connect(config.db.as_ref()).await);

    // Shutdown plumbing: `shutdown` is cancelled once on SIGINT/SIGTERM; every
    // spawned subsystem watches it, and `tasks` tracks their handles so the drain
    // can await in-flight work (a running backup, an unfinished Gary turn) before
    // the process exits — not just close the gateway socket.
    let shutdown = CancellationToken::new();
    let tasks = TaskTracker::new();

    // Builds the backup service (if S3 is configured) and starts its scheduled
    // snapshot cycle; the returned handle also goes into the command Data.
    let backup = setup_backups(
        config.s3.as_ref(),
        config.db.as_ref(),
        config.backup_retention,
        config.backup_interval,
        CycleHandles {
            client: kube_client.clone(),
            http: http.clone(),
            namespace: namespace.clone(),
            domain: domain.clone(),
            control_port,
            catalog: std::sync::Arc::clone(&catalog),
            provision_lock: std::sync::Arc::clone(&provision_lock),
        },
        &tasks,
        shutdown.clone(),
    )
    .await;

    let ollama = build_ollama(
        config.ollama_api_key,
        config.ollama_base_url,
        config.ollama_model,
    );

    // A token-only serenity HTTP client (no gateway) shared by both Gary surfaces
    // to DM operators on escalation. Built here so the in-game endpoint — spawned
    // below, before the gateway client exists — can carry it too.
    let notifier = notify::OperatorNotifier::new(
        std::sync::Arc::new(serenity::Http::new(&config.token)),
        std::sync::Arc::clone(&operator_ids),
    );

    // Start the in-game agent endpoint the game-pod supervisors POST `@Gary` chat
    // triggers to. Shares Gary's core and session store via cloned handles (the
    // same pattern as the backup cycle); stays off when Gary isn't configured.
    ingame::spawn(
        ingame::IngameDeps {
            client: kube_client.clone(),
            http: http.clone(),
            namespace: namespace.clone(),
            domain: domain.clone(),
            control_port,
            catalog: std::sync::Arc::clone(&catalog),
            ollama: ollama.clone(),
            sessions: std::sync::Arc::clone(&sessions),
            notifier: notifier.clone(),
        },
        config.agent_port,
        config.ingame_token,
        &tasks,
        shutdown.clone(),
    );

    let data = Data {
        kube_client,
        http,
        namespace,
        domain,
        control_port,
        catalog,
        provision_lock,
        operator_ids,
        guild_config,
        ollama,
        sessions,
        home_channels,
        memory,
        backup,
        tasks: tasks.clone(),
        notifier,
    };
    run_gateway(config.token, data, shutdown, tasks).await
}

/// Build the poise framework around a pre-constructed [`Data`], connect the
/// Discord client, and run the gateway loop until shutdown. Split from [`run`] so
/// the setup (Kubernetes, catalog, backups, the in-game endpoint) stays readable
/// apart from the gateway wiring.
async fn run_gateway(
    token: String,
    data: Data,
    shutdown: CancellationToken,
    tasks: TaskTracker,
) -> Result<()> {
    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: slash_commands(),
            command_check: Some(|ctx| Box::pin(discord::require_scope(ctx))),
            event_handler: |ctx, event, framework, event_data| {
                Box::pin(on_gateway_event(ctx, event, framework, event_data))
            },
            ..Default::default()
        })
        // The bot is multi-guild: commands register per guild on the GuildCreate
        // event (see on_gateway_event), which fires on startup for every guild the
        // bot is in and again whenever it joins a new one — instant, no ~1h global
        // propagation. So .setup only hands the framework its shared Data.
        .setup(move |_ctx, _ready, _framework| Box::pin(async move { Ok(data) }))
        .build();

    // MESSAGE_CONTENT is privileged (toggle it on in the Discord dev portal).
    // Without it, messages in a home channel arrive with empty content, so Gary
    // could only ever see `@`-mentions and DMs — the two content exemptions.
    let intents =
        serenity::GatewayIntents::non_privileged() | serenity::GatewayIntents::MESSAGE_CONTENT;
    let mut client = serenity::ClientBuilder::new(token, intents)
        .framework(framework)
        .await
        .context("failed to build discord client")?;

    spawn_shutdown_watch(
        std::sync::Arc::clone(&client.shard_manager),
        shutdown.clone(),
    );

    let gateway = client.start().await.context("discord gateway loop failed");

    // The gateway has stopped (a signal fired, or it errored). Cancel the token so
    // any subsystem not already draining begins, then await outstanding work up to
    // the grace window before returning — so a SIGTERM can't cut a Gary turn off
    // between a mutating tool call and its follow-up.
    shutdown.cancel();
    tasks.close();
    if tokio::time::timeout(SHUTDOWN_DRAIN_TIMEOUT, tasks.wait())
        .await
        .is_err()
    {
        warn!(
            timeout_secs = SHUTDOWN_DRAIN_TIMEOUT.as_secs(),
            "drain timed out; exiting with work still in flight"
        );
    } else {
        info!("in-flight work drained cleanly");
    }
    gateway
}

/// Framework event handler: register this guild's slash commands the moment the
/// bot sees it (`GuildCreate` fires for every guild on startup, on each new join,
/// and again on every gateway reconnect/resume), then forward every event to
/// Gary's message handler. Re-registering is idempotent, so the reconnect repeats
/// are harmless. Registering per-guild keeps commands instantly available across
/// any number of guilds, unlike global registration's ~1h propagation.
async fn on_gateway_event(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    framework: poise::FrameworkContext<'_, Data, discord::Error>,
    data: &Data,
) -> Result<(), discord::Error> {
    if let serenity::FullEvent::GuildCreate { guild, .. } = event {
        poise::builtins::register_in_guild(ctx, &framework.options().commands, guild.id).await?;
        // GuildCreate re-fires on every gateway reconnect/resume, so this would be
        // routine repeat noise at info — the interesting signal is a *new* guild,
        // which the join path already surfaces.
        debug!(
            guild = guild.id.get(),
            "registered slash commands for guild"
        );
    }
    discord::gary::on_event(ctx, event, framework, data).await
}

/// On SIGINT/SIGTERM: cancel the shared token (so every subsystem starts
/// draining) and stop the gateway, in a background task. The gateway stopping is
/// what lets [`run_gateway`] fall through to awaiting the tracked work.
fn spawn_shutdown_watch(
    shard_manager: std::sync::Arc<serenity::ShardManager>,
    shutdown: CancellationToken,
) {
    tokio::spawn(async move {
        wait_for_shutdown().await;
        info!("shutdown signal received, draining and stopping discord client");
        shutdown.cancel();
        shard_manager.shutdown_all().await;
    });
}

/// The slash commands the bot registers with each guild it's in.
fn slash_commands() -> Vec<poise::Command<Data, discord::Error>> {
    vec![
        commands::servers(),
        commands::create(),
        commands::shutdown(),
        commands::stop(),
        commands::start(),
        commands::restart(),
        commands::destroy(),
        commands::backup(),
        commands::backups(),
        commands::archive(),
        commands::archives(),
        commands::restore(),
        commands::recover(),
        commands::new_session(),
        commands::gary_home(),
        commands::gary_memory(),
        commands::config(),
    ]
}

/// Build Gary's model connection from config, logging whether the agent is on.
/// `None` (no API key) means mentions are declined with a "not configured" reply.
fn build_ollama(api_key: Option<String>, base_url: String, model: String) -> Option<OllamaConfig> {
    let Some(api_key) = api_key else {
        warn!("OLLAMA_API_KEY not set; agent (Gary) disabled — mentions will be declined");
        return None;
    };
    info!(model = %model, "agent (Gary) enabled");
    Some(OllamaConfig {
        api_key,
        base_url,
        model,
    })
}

/// Owned cluster/catalog handles the scheduled backup cycle's background task
/// needs. Cloned from the same handles that move into the command `Data`.
struct CycleHandles {
    client: kube::Client,
    http: reqwest::Client,
    namespace: String,
    domain: String,
    control_port: u16,
    catalog: std::sync::Arc<agones::GameCatalog>,
    provision_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
}

/// Build the backup service (or `None` when S3 isn't configured) and, when built,
/// start its scheduled snapshot cycle.
async fn setup_backups(
    s3: Option<&config::S3Config>,
    db: Option<&config::DbConfig>,
    retention: usize,
    interval: std::time::Duration,
    handles: CycleHandles,
    tasks: &TaskTracker,
    shutdown: CancellationToken,
) -> backup::MaybeBackups {
    let Some(s3_config) = s3 else {
        warn!("GAMESERVERS_S3_ACCESS_KEY/SECRET_KEY not set; backups/archive/restore disabled");
        return None;
    };
    let service = match backup::BackupService::new(s3_config, db, retention, interval).await {
        Ok(service) => std::sync::Arc::new(service),
        Err(err) => {
            error!(error = ?err, "failed to initialize backups; archive/restore disabled");
            return None;
        }
    };
    info!("backups enabled");
    spawn_backup_cycle(std::sync::Arc::clone(&service), handles, tasks, shutdown);
    Some(service)
}

/// Snapshot every live server on the service's interval, in a tracked background
/// task that stops at the next tick boundary once `shutdown` is cancelled. A
/// snapshot already underway when the signal arrives runs to completion (the
/// drain window bounds the wait), so a backup is never cut mid-stream.
fn spawn_backup_cycle(
    service: std::sync::Arc<backup::BackupService>,
    handles: CycleHandles,
    tasks: &TaskTracker,
    shutdown: CancellationToken,
) {
    let interval = service.interval();
    tasks.spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick fires immediately; skip it so a restart doesn't snapshot
        // right away, only once per interval thereafter.
        ticker.tick().await;
        loop {
            tokio::select! {
                _ = ticker.tick() => {}
                () = shutdown.cancelled() => break,
            }
            let ctx = backup::BackupCtx {
                client: &handles.client,
                http: &handles.http,
                namespace: &handles.namespace,
                domain: &handles.domain,
                control_port: handles.control_port,
                catalog: &handles.catalog,
                provision_lock: &handles.provision_lock,
            };
            service.run_backup_cycle(&ctx).await;
        }
    });
    info!(
        interval_secs = interval.as_secs(),
        "scheduled backup cycle enabled"
    );
}

/// Resolve once SIGINT (Ctrl-C) or SIGTERM is received. SIGTERM is what
/// Kubernetes sends on pod termination, so both must drain the gateway.
async fn wait_for_shutdown() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut sigterm = match signal(SignalKind::terminate()) {
        Ok(stream) => stream,
        Err(err) => {
            error!(error = %err, "failed to install SIGTERM handler; relying on SIGINT only");
            if let Err(ctrl_c_err) = tokio::signal::ctrl_c().await {
                error!(error = %ctrl_c_err, "failed to listen for SIGINT");
            }
            return;
        }
    };

    tokio::select! {
        result = tokio::signal::ctrl_c() => {
            if let Err(err) = result {
                error!(error = %err, "failed to listen for SIGINT");
            }
        }
        _ = sigterm.recv() => {}
    }
}
