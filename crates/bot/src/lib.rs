mod admin;
mod agent;
mod agones;
mod backup;
mod capacity;
mod config;
mod cycles;
mod defer;
mod discord;
mod domain;
mod events;
mod http;
mod ingame;
mod interrupt;
mod kv;
mod limits;
mod mail;
mod memory;
mod modrinth;
mod notify;
mod prompts;
mod registry;
mod settings;
mod signups;
mod sizing;
mod store;
mod telemetry;
mod throttle;
mod thunderstore;
mod untrusted;
mod watchdog;

pub use config::BotConfig;

use anyhow::{Context as _, Result};
use poise::serenity_prelude as serenity;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{debug, error, info, warn};

use agent::{OllamaConfig, SessionStore};
use discord::Data;
use memory::GaryMemory;
use store::{GuildConfig, HomeChannels};

const SUPERVISOR_HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// How long, after the gateway stops, to let in-flight work (Gary sessions, a
/// running backup cycle, in-game answers) finish before the process exits anyway.
/// Kubernetes' default `terminationGracePeriodSeconds` is 30s, so stay under it.
const SHUTDOWN_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

pub async fn run(mut config: BotConfig) -> Result<()> {
    let kube_client = kube::Client::try_default()
        .await
        .context("failed to initialize kubernetes client")?;

    let catalog = load_game_catalog(&config.catalog_dir).await?;

    let (http, control_http) = build_http_clients(config.supervisor_token.as_deref())?;

    let shutdown = CancellationToken::new();
    let tasks = TaskTracker::new();

    let telemetry = setup_telemetry(&config, http.clone(), &tasks, shutdown.clone());

    let settings = connect_settings(&config).await;
    let effective = settings.snapshot();

    let listeners = Listeners::take(&mut config);
    let namespace = config.namespace;
    let domain = config.domain;
    let control_port = config.control_port;
    let operator_ids: std::sync::Arc<[u64]> = config.operator_ids.into();
    let provision_lock = std::sync::Arc::new(tokio::sync::Mutex::new(()));
    let DurableState {
        home_channels,
        guild_config,
        memory,
        sizing,
        guild_limits,
        events,
        signups,
    } = DurableState::connect(config.db.as_ref(), std::sync::Arc::clone(&settings)).await;
    let defer = std::sync::Arc::new(defer::DeferRuntime::new(
        kv::ValkeyClient::connect(config.valkey.as_ref()).await,
    ));

    let (watchdog_tx, watchdog_rx) = watchdog::channel();

    let (backup, cycle_health) = setup_cycles(
        CycleSchedule::from_settings(&effective),
        BackupInputs {
            s3: config.s3.as_ref(),
            db: config.db.as_ref(),
            supervisor_token: config.supervisor_token.as_deref(),
        },
        CycleHandles {
            client: kube_client.clone(),
            http: control_http.clone(),
            namespace: namespace.clone(),
            domain: domain.clone(),
            control_port,
            catalog: std::sync::Arc::clone(&catalog),
            provision_lock: std::sync::Arc::clone(&provision_lock),
            events: std::sync::Arc::clone(&events),
            sizing: std::sync::Arc::clone(&sizing),
            guild_limits: std::sync::Arc::clone(&guild_limits),
        },
        watchdog_tx,
        std::sync::Arc::clone(&settings),
        &tasks,
        shutdown.clone(),
    )
    .await;

    let ollama = build_ollama(config.ollama_api_key, &effective);

    let notifier = build_notifier(&config.token, &operator_ids);

    let mailer = std::sync::Arc::new(mail::Mailer::connect(config.mail.as_ref()));

    let data = Data {
        kube_client,
        http,
        control_http,
        namespace,
        domain,
        control_port,
        catalog,
        provision_lock,
        operator_ids,
        settings: std::sync::Arc::clone(&settings),
        guild_config,
        ollama,
        sessions: std::sync::Arc::new(SessionStore::new()),
        home_channels,
        memory,
        sizing,
        guild_limits,
        backup,
        turn_throttle: std::sync::Arc::new(throttle::TurnThrottle::new(effective.throttle)),
        interrupts: std::sync::Arc::new(interrupt::TurnInterrupts::default()),
        tasks: tasks.clone(),
        notifier,
        defer,
        shutdown: shutdown.clone(),
        gateway: discord::GatewayHandle::new(),
        environment: config.environment,
        telemetry: std::sync::Arc::clone(&telemetry),
        events,
        signups,
        mailer,
    };

    watchdog::spawn_responder(data.clone(), watchdog_rx, &cycle_health);

    spawn_listeners(&data, listeners, cycle_health, &tasks, &shutdown);

    run_gateway(config.token, data, shutdown, tasks, telemetry).await
}

struct Listeners {
    agent_port: u16,
    tokens: http::EndpointTokens,
    admin: Option<config::AdminConfig>,
}

impl Listeners {
    fn take(config: &mut BotConfig) -> Self {
        Self {
            agent_port: config.agent_port,
            tokens: http::EndpointTokens {
                ingame: config.ingame_token.take(),
                operator: config.operator_token.take(),
                signup: config.signup_token.take(),
            },
            admin: config.admin.take(),
        }
    }
}

fn spawn_listeners(
    data: &Data,
    listeners: Listeners,
    cycle_health: std::sync::Arc<cycles::CycleHealth>,
    tasks: &tokio_util::task::TaskTracker,
    shutdown: &tokio_util::sync::CancellationToken,
) {
    let Listeners {
        agent_port,
        tokens,
        admin,
    } = listeners;
    let deps = http::HttpDeps::from_data(data, agent_port, tokens, cycle_health);
    http::spawn(deps, tasks, shutdown.clone());
    admin::spawn(data, admin, tasks, shutdown.clone());
}

async fn connect_settings(config: &BotConfig) -> std::sync::Arc<settings::RuntimeSettings> {
    let settings = std::sync::Arc::new(
        settings::RuntimeSettings::connect(
            config.db.as_ref(),
            settings::SettingsSnapshot::baseline(config),
        )
        .await,
    );
    announce_guild_allowlist(&settings.snapshot().guild_allowlist);
    settings
}

fn announce_guild_allowlist(guilds: &[u64]) {
    if guilds.is_empty() {
        warn!("no guild allowlist set: every guild the bot is invited to is served");
    } else {
        info!(guilds = guilds.len(), "serving only allowlisted guilds");
    }
}

fn setup_telemetry(
    config: &BotConfig,
    http: reqwest::Client,
    tasks: &TaskTracker,
    shutdown: CancellationToken,
) -> std::sync::Arc<telemetry::Telemetry> {
    let (telemetry, score_poster) = telemetry::Telemetry::new(
        config.langfuse.as_ref(),
        config.release.clone(),
        http,
        shutdown,
    );
    tasks.spawn(score_poster);
    std::sync::Arc::new(telemetry)
}

async fn run_gateway(
    token: String,
    data: Data,
    shutdown: CancellationToken,
    tasks: TaskTracker,
    telemetry: std::sync::Arc<telemetry::Telemetry>,
) -> Result<()> {
    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: discord::slash_commands(),
            command_check: Some(|ctx| Box::pin(discord::require_scope(ctx))),
            event_handler: |ctx, event, framework, event_data| {
                Box::pin(on_gateway_event(ctx, event, framework, event_data))
            },
            ..Default::default()
        })
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
    telemetry.drain().await;
    gateway
}

async fn on_gateway_event(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    framework: poise::FrameworkContext<'_, Data, discord::Error>,
    data: &Data,
) -> Result<(), discord::Error> {
    if let serenity::FullEvent::GuildCreate { guild, .. } = event {
        let admitted = discord::guild_is_admitted(data, guild.id.get());
        discord::register_guild_commands(
            ctx,
            framework.options().commands.as_slice(),
            guild.id,
            admitted,
        )
        .await?;
        if !admitted {
            warn!(
                guild = guild.id.get(),
                "ignoring a guild that is not on the allowlist"
            );
            return Ok(());
        }
        debug!(
            guild = guild.id.get(),
            "registered slash commands for guild"
        );
        if data.events.claim_guild(guild.id.get()).await {
            info!(guild = guild.id.get(), "joined a new guild");
            data.events
                .record(
                    events::ProductEvent::new(
                        events::EventKind::GuildJoined,
                        &events::Attribution::system(),
                        events::mapping::JOINED,
                    )
                    .guild(Some(domain::GuildId::new(guild.id.get().to_string()))),
                )
                .await;
        }
    }
    if let serenity::FullEvent::Ready { data_about_bot, .. } = event {
        let guilds: Vec<u64> = data_about_bot
            .guilds
            .iter()
            .map(|guild| guild.id.get())
            .collect();
        data.events.seed_known_guilds(&guilds).await;
        data.gateway.publish(ctx);
        data.defer.reconcile(data, ctx).await;
    }
    discord::gary::on_event(ctx, event, framework, data).await
}

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

fn build_notifier(token: &str, operator_ids: &std::sync::Arc<[u64]>) -> notify::OperatorNotifier {
    notify::OperatorNotifier::new(
        std::sync::Arc::new(serenity::Http::new(token)),
        std::sync::Arc::clone(operator_ids),
    )
}

fn build_ollama(
    api_key: Option<String>,
    effective: &settings::SettingsSnapshot,
) -> Option<OllamaConfig> {
    let Some(api_key) = api_key else {
        warn!("OLLAMA_API_KEY not set; agent (Gary) disabled — mentions will be declined");
        return None;
    };
    info!(model = %effective.gary_model, "agent (Gary) enabled");
    Some(OllamaConfig {
        api_key,
        base_url: effective.gary_base_url.clone(),
        model: effective.gary_model.clone(),
    })
}

#[derive(Clone)]
struct CycleHandles {
    client: kube::Client,
    http: agones::ControlClient,
    namespace: String,
    domain: String,
    control_port: u16,
    catalog: std::sync::Arc<agones::GameCatalog>,
    provision_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
    events: std::sync::Arc<events::EventLog>,
    sizing: std::sync::Arc<sizing::InstanceSizing>,
    guild_limits: std::sync::Arc<limits::GuildLimits>,
}

struct CycleSchedule {
    backup_interval: std::time::Duration,
    backup_retention: usize,
    archive_retention_days: i32,
    reconcile_interval: std::time::Duration,
    stranded_dwell: std::time::Duration,
    sample_interval: std::time::Duration,
}

impl CycleSchedule {
    fn from_settings(effective: &settings::SettingsSnapshot) -> Self {
        Self {
            backup_interval: effective.backup_interval,
            backup_retention: effective.backup_retention,
            archive_retention_days: effective.archive_retention_days,
            reconcile_interval: effective.reconcile_interval,
            stranded_dwell: effective.stranded_dwell,
            sample_interval: effective.sample_interval,
        }
    }
}

async fn load_game_catalog(dir: &std::path::Path) -> Result<std::sync::Arc<agones::GameCatalog>> {
    let catalog = agones::load_catalog(dir)
        .await
        .with_context(|| format!("failed to load game catalog from {}", dir.display()))?;
    Ok(std::sync::Arc::new(catalog))
}

struct DurableState {
    home_channels: std::sync::Arc<HomeChannels>,
    guild_config: std::sync::Arc<GuildConfig>,
    memory: std::sync::Arc<GaryMemory>,
    sizing: std::sync::Arc<sizing::InstanceSizing>,
    guild_limits: std::sync::Arc<limits::GuildLimits>,
    events: std::sync::Arc<events::EventLog>,
    signups: std::sync::Arc<signups::Signups>,
}

impl DurableState {
    async fn connect(
        db: Option<&config::DbConfig>,
        settings: std::sync::Arc<settings::RuntimeSettings>,
    ) -> Self {
        Self {
            home_channels: std::sync::Arc::new(HomeChannels::connect(db).await),
            guild_config: std::sync::Arc::new(GuildConfig::connect(db).await),
            memory: std::sync::Arc::new(GaryMemory::connect(db).await),
            sizing: std::sync::Arc::new(sizing::InstanceSizing::connect(db).await),
            guild_limits: std::sync::Arc::new(limits::GuildLimits::connect(db, settings).await),
            events: std::sync::Arc::new(events::EventLog::connect(db).await),
            signups: std::sync::Arc::new(signups::Signups::connect(db).await),
        }
    }
}

#[derive(Clone)]
struct ReconcileHandles {
    client: kube::Client,
    namespace: String,
    domain: String,
    catalog: std::sync::Arc<agones::GameCatalog>,
    events: std::sync::Arc<events::EventLog>,
    sizing: std::sync::Arc<sizing::InstanceSizing>,
    dwell: std::time::Duration,
}

impl ReconcileHandles {
    fn from_cycle(handles: &CycleHandles, dwell: std::time::Duration) -> Self {
        Self {
            client: handles.client.clone(),
            namespace: handles.namespace.clone(),
            domain: handles.domain.clone(),
            catalog: std::sync::Arc::clone(&handles.catalog),
            events: std::sync::Arc::clone(&handles.events),
            sizing: std::sync::Arc::clone(&handles.sizing),
            dwell,
        }
    }
}

async fn setup_cycles(
    schedule: CycleSchedule,
    backups: BackupInputs<'_>,
    handles: CycleHandles,
    watchdog: tokio::sync::mpsc::Sender<watchdog::Incident>,
    settings: std::sync::Arc<settings::RuntimeSettings>,
    tasks: &TaskTracker,
    shutdown: CancellationToken,
) -> (backup::MaybeBackups, std::sync::Arc<cycles::CycleHealth>) {
    let health = &cycles::CycleHealth::new();
    spawn_stranded_cycle(
        ReconcileHandles::from_cycle(&handles, schedule.stranded_dwell),
        schedule.reconcile_interval,
        health,
        tasks,
        shutdown.clone(),
    );
    spawn_sampler(
        SamplerHandles::from_cycle(&handles),
        watchdog,
        settings,
        schedule.sample_interval,
        health,
        tasks,
        shutdown.clone(),
    );
    let service = setup_backups(
        backups,
        Retention {
            backups: schedule.backup_retention,
            archive_days: schedule.archive_retention_days,
        },
        schedule.backup_interval,
        handles,
        health,
        tasks,
        shutdown,
    )
    .await;
    (service, std::sync::Arc::clone(health))
}

struct BackupInputs<'a> {
    s3: Option<&'a config::S3Config>,
    db: Option<&'a config::DbConfig>,
    supervisor_token: Option<&'a str>,
}

fn build_http_clients(token: Option<&str>) -> Result<(reqwest::Client, agones::ControlClient)> {
    let http = reqwest::Client::builder()
        .timeout(SUPERVISOR_HTTP_TIMEOUT)
        .build()
        .context("failed to build supervisor control http client")?;
    let control_http = agones::ControlClient::new(http.clone(), token);
    if control_http.is_authenticated() {
        info!("supervisor control calls are authenticated");
    } else {
        warn!(
            "SUPERVISOR_CONTROL_TOKEN not set; supervisor control calls rely on network policy alone"
        );
    }
    Ok((http, control_http))
}

struct Retention {
    backups: usize,
    archive_days: i32,
}

fn spawn_stranded_cycle(
    handles: ReconcileHandles,
    interval: std::time::Duration,
    health: &std::sync::Arc<cycles::CycleHealth>,
    tasks: &TaskTracker,
    shutdown: CancellationToken,
) {
    let cancel = shutdown.clone();
    cycles::spawn_supervised("stranded-reconciler", health, tasks, shutdown, move || {
        let handles = handles.clone();
        let stop = cancel.clone();
        async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticker.tick().await;
            loop {
                tokio::select! {
                    _ = ticker.tick() => {}
                    () = stop.cancelled() => break,
                }
                let ctx = agones::StrandedCtx {
                    client: &handles.client,
                    namespace: &handles.namespace,
                    domain: &handles.domain,
                    catalog: &handles.catalog,
                    events: &handles.events,
                    sizing: &handles.sizing,
                };
                agones::run_stranded_cycle(&ctx, handles.dwell).await;
            }
        }
    });
    info!(
        interval_secs = interval.as_secs(),
        "stranded-server reconciler enabled"
    );
}

#[derive(Clone)]
struct SamplerHandles {
    client: kube::Client,
    http: agones::ControlClient,
    namespace: String,
    control_port: u16,
    events: std::sync::Arc<events::EventLog>,
}

impl SamplerHandles {
    fn from_cycle(handles: &CycleHandles) -> Self {
        Self {
            client: handles.client.clone(),
            http: handles.http.clone(),
            namespace: handles.namespace.clone(),
            control_port: handles.control_port,
            events: std::sync::Arc::clone(&handles.events),
        }
    }
}

fn spawn_sampler(
    handles: SamplerHandles,
    watchdog: tokio::sync::mpsc::Sender<watchdog::Incident>,
    settings: std::sync::Arc<settings::RuntimeSettings>,
    interval: std::time::Duration,
    health: &std::sync::Arc<cycles::CycleHealth>,
    tasks: &TaskTracker,
    shutdown: CancellationToken,
) {
    if !handles.events.enabled() {
        warn!("no database; occupancy and resource sampling disabled (nothing else changes)");
        warn!("watchdog detection rides the sampler, so it is disabled too");
        return;
    }
    let cancel = shutdown.clone();
    cycles::spawn_supervised("sampler", health, tasks, shutdown, move || {
        let handles = handles.clone();
        let watchdog = watchdog.clone();
        let settings = std::sync::Arc::clone(&settings);
        let stop = cancel.clone();
        async move {
            let mut detector: Option<watchdog::DetectorState> = None;
            let mut reported_unlabelled = events::ReportedUnlabelledPvcs::default();
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticker.tick().await;
            loop {
                tokio::select! {
                    _ = ticker.tick() => {}
                    () = stop.cancelled() => break,
                }
                let ctx = events::SamplerCtx {
                    client: &handles.client,
                    http: &handles.http,
                    namespace: &handles.namespace,
                    control_port: handles.control_port,
                    events: &handles.events,
                };
                let observations = events::run_occupancy_cycle(&ctx).await;
                let resources = events::run_resource_cycle(&ctx, &mut reported_unlabelled).await;
                if settings.snapshot().watchdog == config::WatchdogMode::Off {
                    detector = None;
                    continue;
                }
                let state = detector.get_or_insert_with(|| watchdog::DetectorState::new(interval));
                let fired = state.tick(std::time::Instant::now(), &observations, &resources);
                for incident in fired {
                    forward_incident(&watchdog, incident);
                }
            }
        }
    });
    info!(
        interval_secs = interval.as_secs(),
        "occupancy and resource sampling enabled"
    );
}

fn forward_incident(
    sender: &tokio::sync::mpsc::Sender<watchdog::Incident>,
    incident: watchdog::Incident,
) {
    use tokio::sync::mpsc::error::TrySendError;
    match sender.try_send(incident) {
        Ok(()) => {}
        Err(TrySendError::Full(incident)) => warn!(
            instance = %incident.instance,
            signal = incident.kind.as_str(),
            "watchdog incident queue full; dropping incident (re-fires after cooldown)"
        ),
        Err(TrySendError::Closed(incident)) => warn!(
            instance = %incident.instance,
            signal = incident.kind.as_str(),
            "watchdog responder gone; dropping incident"
        ),
    }
}

async fn setup_backups(
    inputs: BackupInputs<'_>,
    retention: Retention,
    interval: std::time::Duration,
    handles: CycleHandles,
    health: &std::sync::Arc<cycles::CycleHealth>,
    tasks: &TaskTracker,
    shutdown: CancellationToken,
) -> backup::MaybeBackups {
    let BackupInputs {
        s3,
        db,
        supervisor_token,
    } = inputs;
    let Some(s3_config) = s3 else {
        warn!("GAMESERVERS_S3_ACCESS_KEY/SECRET_KEY not set; backups/archive/restore disabled");
        return None;
    };
    let events = std::sync::Arc::clone(&handles.events);
    let service = match backup::BackupService::new(
        s3_config,
        db,
        retention.backups,
        retention.archive_days,
        supervisor_token,
        interval,
        events,
    )
    .await
    {
        Ok(service) => std::sync::Arc::new(service),
        Err(err) => {
            error!(error = ?err, "failed to initialize backups; archive/restore disabled");
            return None;
        }
    };
    info!("backups enabled");
    spawn_backup_cycle(
        std::sync::Arc::clone(&service),
        handles,
        health,
        tasks,
        shutdown,
    );
    Some(service)
}

fn spawn_backup_cycle(
    service: std::sync::Arc<backup::BackupService>,
    handles: CycleHandles,
    health: &std::sync::Arc<cycles::CycleHealth>,
    tasks: &TaskTracker,
    shutdown: CancellationToken,
) {
    let interval = service.interval();
    let cancel = shutdown.clone();
    cycles::spawn_supervised("backup-cycle", health, tasks, shutdown, move || {
        let service = std::sync::Arc::clone(&service);
        let handles = handles.clone();
        let stop = cancel.clone();
        async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticker.tick().await;
            loop {
                tokio::select! {
                    _ = ticker.tick() => {}
                    () = stop.cancelled() => break,
                }
                let ctx = backup::BackupCtx {
                    client: &handles.client,
                    http: &handles.http,
                    namespace: &handles.namespace,
                    domain: &handles.domain,
                    control_port: handles.control_port,
                    catalog: &handles.catalog,
                    provision_lock: &handles.provision_lock,
                    sizing: &handles.sizing,
                    guild_limits: &handles.guild_limits,
                };
                service.run_backup_cycle(&ctx).await;
            }
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
