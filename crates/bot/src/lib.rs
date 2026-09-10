//! `grizzly-gameservers`: the Discord shim, ops agent, and Agones client for
//! friends to spin up and manage game servers. [`discord`] owns the slash
//! commands and Gary's Discord-facing shell; [`agent`] is Gary's reusable
//! chat-completions/tool-calling core; [`agones`] talks to Kubernetes and
//! Agones. [`run`] wires them together and drives the gateway loop.

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
pub async fn run(mut config: BotConfig) -> Result<()> {
    let kube_client = kube::Client::try_default()
        .await
        .context("failed to initialize kubernetes client")?;

    let catalog = load_game_catalog(&config.catalog_dir).await?;

    let (http, control_http) = build_http_clients(config.supervisor_token.as_deref())?;

    // Shutdown plumbing: `shutdown` is cancelled once on SIGINT/SIGTERM; every
    // spawned subsystem watches it, and `tasks` tracks their handles so the drain
    // can await in-flight work (a running backup, an unfinished Gary turn) before
    // the process exits — not just close the gateway socket.
    let shutdown = CancellationToken::new();
    let tasks = TaskTracker::new();

    // Both Gary surfaces record through this one exporter, so it is built before
    // anything that carries it.
    let telemetry = setup_telemetry(&config, http.clone(), &tasks, shutdown.clone());

    // The operator-tunable knobs, resolved database override > environment >
    // compiled default. Built before everything that reads a tunable value, so
    // the subsystems below are wired from the *effective* figures rather than
    // from `BotConfig` directly — that is what makes a stored override real on
    // this boot rather than only after the next deploy.
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
    // Gary's deferred-task queue on the foundation kv-cache (Valkey). Disabled
    // gracefully when REDIS_PASSWORD isn't set — run_when then reports it can't
    // schedule, the same shape as db/s3/ollama.
    let defer = std::sync::Arc::new(defer::DeferRuntime::new(
        kv::ValkeyClient::connect(config.valkey.as_ref()).await,
    ));

    // The watchdog's sampler→responder seam: detection rides the sampler task
    // (spawned below, before `Data` exists), the responder holds `Data` and is
    // spawned after it. Wired whatever the mode says, because the mode is a
    // runtime setting — both ends read it live.
    let (watchdog_tx, watchdog_rx) = watchdog::channel();

    // Starts the scheduled background work — stranded-server recovery always,
    // world backups when S3 is configured. The backup handle it returns also
    // goes into the command Data; the health handle goes to the probes.
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

    // A token-only serenity HTTP client (no gateway) shared by both Gary surfaces
    // to DM operators on escalation. Built here so the in-game endpoint — spawned
    // below, before the gateway client exists — can carry it too.
    let notifier = build_notifier(&config.token, &operator_ids);

    // Approval mail. Not part of DurableState: it is SMTP, not Postgres, and
    // shares neither its credential nor its failure mode.
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

    // The watchdog responder consumes fired incidents with the full `Data` in
    // hand; spawned here because `Data` now exists, before the gateway so no
    // incident is lost while Discord connects (it waits for a Context itself).
    watchdog::spawn_responder(data.clone(), watchdog_rx, &cycle_health);

    spawn_listeners(&data, listeners, cycle_health, &tasks, &shutdown);

    run_gateway(config.token, data, shutdown, tasks, telemetry).await
}

/// Where the process listens, beyond the Discord gateway, and the credentials
/// each surface expects.
struct Listeners {
    agent_port: u16,
    tokens: http::EndpointTokens,
    admin: Option<config::AdminConfig>,
}

impl Listeners {
    /// Take the listener settings out of the config, so `run` carries one value
    /// rather than five loose fields it must not transpose.
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

/// Bring up both inbound HTTP surfaces.
///
/// Both carve their dependencies out of the command `Data` (the same pattern as
/// the backup cycle), so this must run after `Data` exists — and before the
/// gateway, so the k8s probes have a `/health` target while Discord is still
/// connecting.
///
/// They are separate listeners on purpose: the agent port has to stay reachable
/// by game pods, which run third-party server binaries, while the panel trusts
/// forward-auth headers and so must admit only the ingress controller. One port
/// cannot carry both claims.
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

/// Resolve this boot's settings — stored overrides layered onto the environment
/// values — and announce the admission posture they resolve to.
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

/// Say at startup which admission posture is in force. An empty list is the open
/// posture and is worth a `warn`: it is correct for a deploy that hasn't been
/// given a list, and wrong for one that was meant to have one, and only the log
/// distinguishes them. Reports the *effective* list, so an operator-set override
/// is what's announced rather than whatever the environment happened to carry.
fn announce_guild_allowlist(guilds: &[u64]) {
    if guilds.is_empty() {
        warn!("no guild allowlist set: every guild the bot is invited to is served");
    } else {
        info!(guilds = guilds.len(), "serving only allowlisted guilds");
    }
}

/// Stand up turn telemetry and put its score poster on the tracked task set, so
/// queued scores are flushed by the shutdown drain rather than dropped.
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

/// Build the poise framework around a pre-constructed [`Data`], connect the
/// Discord client, and run the gateway loop until shutdown. Split from [`run`] so
/// the setup (Kubernetes, catalog, backups, the in-game endpoint) stays readable
/// apart from the gateway wiring.
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
    // Only now: a turn still finishing inside the drain above is still emitting
    // spans, so flushing any earlier would ship an incomplete trace and lose the
    // rest. `Data` is gone by here, leaving this the last handle.
    telemetry.drain().await;
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
        // An unadmitted guild gets an *empty* command set rather than being
        // skipped: a guild removed from the allowlist must lose the commands it
        // was already registered with, and re-registering empty is how that
        // revocation reaches Discord.
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
        // GuildCreate re-fires on every gateway reconnect/resume, so this would be
        // routine repeat noise at info — the interesting signal is a *new* guild,
        // which the join path already surfaces.
        debug!(
            guild = guild.id.get(),
            "registered slash commands for guild"
        );
        // Only a guild the store has never seen counts as an acquisition; the
        // reconnect replays land on an existing row and say nothing.
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
        // Reconcile the known-guilds set before anything else can interpret a
        // GuildCreate as a join: the guilds we're already in are known, not newly
        // acquired, so the acquisition timeline starts at deployment rather than
        // backdating every guild the bot was in.
        let guilds: Vec<u64> = data_about_bot
            .guilds
            .iter()
            .map(|guild| guild.id.get())
            .collect();
        data.events.seed_known_guilds(&guilds).await;
        // Publish next: it's a single store, and it's what unblocks the HTTP
        // routes that can't act until a Context exists. Reconciling can take a
        // while (Valkey round-trips plus a watcher spawn per pending task).
        data.gateway.publish(ctx);
        // A Context now exists, so rebuild deferred-task watchers from Valkey and
        // let a pending run_when survive the redeploy that just happened.
        data.defer.reconcile(data, ctx).await;
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

/// Build the operator DM notifier on its own token-only serenity `Http` client
/// (no gateway), so surfaces spawned before the gateway exists can carry it.
fn build_notifier(token: &str, operator_ids: &std::sync::Arc<[u64]>) -> notify::OperatorNotifier {
    notify::OperatorNotifier::new(
        std::sync::Arc::new(serenity::Http::new(token)),
        std::sync::Arc::clone(operator_ids),
    )
}

/// Build Gary's model connection, logging whether the agent is on. `None` (no
/// API key) means mentions are declined with a "not configured" reply.
///
/// The key is the startup-frozen half — it is a credential, so it stays in the
/// environment and rotating it is a redeploy — while the model and base URL come
/// from the settings snapshot and are re-read per turn. This value is therefore
/// the *boot* connection: whether Gary exists at all, plus the endpoint he
/// started with. See [`discord::Data::ollama_now`].
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

/// Owned cluster/catalog handles the scheduled backup cycle's background task
/// needs. Cloned from the same handles that move into the command `Data`.
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

/// Cadences and budgets for the scheduled background cycles.
struct CycleSchedule {
    backup_interval: std::time::Duration,
    backup_retention: usize,
    archive_retention_days: i32,
    reconcile_interval: std::time::Duration,
    stranded_dwell: std::time::Duration,
    sample_interval: std::time::Duration,
}

impl CycleSchedule {
    /// Read from the resolved settings rather than from `BotConfig`, so an
    /// override the operator recorded is in force from this boot. Each value is
    /// captured into a ticker below and not re-read, which is why these keys are
    /// the restart-class ones in `crate::settings`.
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

/// Load the per-game catalog, naming the directory in the failure — a missing or
/// malformed catalog is the one startup error most likely to be a deploy mistake
/// rather than a bug, so the path has to be in the message.
async fn load_game_catalog(dir: &std::path::Path) -> Result<std::sync::Arc<agones::GameCatalog>> {
    let catalog = agones::load_catalog(dir)
        .await
        .with_context(|| format!("failed to load game catalog from {}", dir.display()))?;
    Ok(std::sync::Arc::new(catalog))
}

/// Everything the bot keeps on foundation Postgres. Bundled because they share
/// one credential and one failure mode — each connects independently and each
/// degrades to a disabled façade rather than failing the boot, so the absence of
/// a database costs features, never the process.
struct DurableState {
    home_channels: std::sync::Arc<HomeChannels>,
    guild_config: std::sync::Arc<GuildConfig>,
    memory: std::sync::Arc<GaryMemory>,
    sizing: std::sync::Arc<sizing::InstanceSizing>,
    guild_limits: std::sync::Arc<limits::GuildLimits>,
    /// The product plane. Every layer that executes a lifecycle, backup, or
    /// escalation action writes here.
    events: std::sync::Arc<events::EventLog>,
    /// The beta signup queue the public landing page feeds and the admin panel
    /// works through.
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

/// Owned cluster/catalog handles the stranded-instance reconciler needs. It
/// reaches only the Kubernetes API, so it carries none of the backup cycle's
/// HTTP/supervisor handles.
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

/// Start every scheduled background cycle, returning the backup service the
/// commands also need.
///
/// Stranded-server recovery starts unconditionally: unlike backups it depends on
/// no optional configuration, and the failure it exists to catch is precisely
/// the one nobody is around to notice.
async fn setup_cycles(
    schedule: CycleSchedule,
    backups: BackupInputs<'_>,
    handles: CycleHandles,
    watchdog: tokio::sync::mpsc::Sender<watchdog::Incident>,
    settings: std::sync::Arc<settings::RuntimeSettings>,
    tasks: &TaskTracker,
    shutdown: CancellationToken,
) -> (backup::MaybeBackups, std::sync::Arc<cycles::CycleHealth>) {
    // Shared by every supervised cycle and by the health endpoint: a cycle that
    // keeps panicking is the one thing a liveness probe can act on that a log
    // line can't.
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

/// What the backups subsystem is built from: where artifacts live, where the
/// archive index lives, and the credential for reaching game pods.
struct BackupInputs<'a> {
    s3: Option<&'a config::S3Config>,
    db: Option<&'a config::DbConfig>,
    supervisor_token: Option<&'a str>,
}

/// The bot's two outbound HTTP clients, sharing one transport.
///
/// They differ only in type, and that is the point: the returned
/// [`agones::ControlClient`] carries the shared control credential, so it can only
/// ever ride a request aimed at a game pod. The bare client still reaches Ollama,
/// Modrinth, and Langfuse — third parties that must never see the token — and the
/// compiler is what keeps the two apart.
///
/// # Errors
///
/// Returns an error if the underlying client can't be built.
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

/// How much backup and archive history to keep. Bundled so the two knobs travel
/// together — they are read from the same config and consumed by the same cycle.
struct Retention {
    /// Backups kept per server before older ones are pruned.
    backups: usize,
    /// Days an archive is kept, never applied to the newest of a name.
    archive_days: i32,
}

/// Sweep for servers stranded on a node that stopped answering and bring them
/// back on a healthy one, in a tracked background task that stops at the next
/// tick boundary once `shutdown` is cancelled.
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
            // The first tick fires immediately; skip it so a redeploy doesn't sweep
            // before the cluster has settled, only once per interval thereafter.
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

/// Owned handles the sampler needs: the cluster and supervisor reach to take a
/// reading, and the store to append it to.
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

/// Record each running server's player count and restart counter, and every
/// existing instance's resource footprint, on the sampler interval — in a
/// tracked background task that stops at the next tick boundary once `shutdown`
/// is cancelled. When the watchdog is configured, each tick's observations also
/// feed its detector, and fired incidents cross to the responder.
///
/// Unlike the stranded reconciler this does *not* start unconditionally: without
/// a database the samples have nowhere to land, so polling every supervisor
/// forever would be pure cluster chatter for no record.
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
            // Absent while the watchdog is off, and rebuilt from scratch when it
            // comes back on: the detector works by differencing each tick against
            // the last one it saw, so carrying state across a gap would read the
            // whole quiet period as a crash burst the moment detection resumed.
            // A restart after a panic is such a gap, and starts it over for the
            // same reason.
            let mut detector: Option<watchdog::DetectorState> = None;
            let mut reported_unlabelled = events::ReportedUnlabelledPvcs::default();
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // The first tick fires immediately; skip it so a redeploy doesn't
            // sample before the cluster has settled, only once per interval after.
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
                // Sampling itself is unconditional — the occupancy and cost series
                // are wanted whatever the watchdog is doing. Only detection follows
                // the mode, which is read here so a change binds on the next tick.
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

/// Hand a fired incident to the responder without blocking the sampler. A full
/// queue means the responder is wedged mid-turn; dropping is bounded loss (a
/// still-broken server re-fires after its cooldown) and is logged loudly.
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

/// Build the backup service (or `None` when S3 isn't configured) and, when built,
/// start its scheduled snapshot cycle.
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

/// Snapshot every live server on the service's interval, in a tracked background
/// task that stops at the next tick boundary once `shutdown` is cancelled. A
/// snapshot already underway when the signal arrives runs to completion (the
/// drain window bounds the wait), so a backup is never cut mid-stream.
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
            // The first tick fires immediately; skip it so a restart doesn't
            // snapshot right away, only once per interval thereafter.
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
