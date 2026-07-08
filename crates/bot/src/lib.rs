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
mod ingame;
mod store;

pub use config::BotConfig;

use anyhow::{Context as _, Result};
use poise::serenity_prelude as serenity;
use tracing::{error, info, warn};

use agent::{OllamaConfig, SessionStore};
use discord::{Data, commands};
use store::HomeChannels;

/// Start the Discord bot: connect to Kubernetes, register the guild-scoped
/// slash commands, and run the gateway loop until a shutdown signal arrives.
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

    // Short default: the supervisor control API is one in-cluster hop away, so a
    // slow response usually means a stuck pod, not a far server. The mutating
    // stop/restart calls override this per-request (they block on the in-pod
    // graceful stop) — see CONTROL_MUTATION_TIMEOUT in agones::supervisor.
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .context("failed to build supervisor control http client")?;

    let namespace = config.namespace;
    let domain = config.domain;
    let control_port = config.control_port;
    let admin_role_id = config.admin_role_id;
    let admin_user_ids: std::sync::Arc<[u64]> = config.admin_user_ids.into();
    let provision_lock = std::sync::Arc::new(tokio::sync::Mutex::new(()));
    let sessions = std::sync::Arc::new(SessionStore::new());
    let home_channels = std::sync::Arc::new(HomeChannels::connect(config.db.as_ref()).await);

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
    )
    .await;

    let ollama = build_ollama(
        config.ollama_api_key,
        config.ollama_base_url,
        config.ollama_model,
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
        },
        config.agent_port,
        config.ingame_token,
    );

    let data = Data {
        kube_client,
        http,
        namespace,
        domain,
        control_port,
        catalog,
        provision_lock,
        admin_role_id,
        admin_user_ids,
        ollama,
        sessions,
        home_channels,
        backup,
    };
    run_gateway(config.token, serenity::GuildId::new(config.guild_id), data).await
}

/// Build the poise framework around a pre-constructed [`Data`], connect the
/// Discord client, and run the gateway loop until shutdown. Split from [`run`] so
/// the setup (Kubernetes, catalog, backups, the in-game endpoint) stays readable
/// apart from the gateway wiring.
async fn run_gateway(token: String, guild_id: serenity::GuildId, data: Data) -> Result<()> {
    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: slash_commands(),
            command_check: Some(|ctx| Box::pin(discord::require_scope(ctx))),
            event_handler: |ctx, event, framework, event_data| {
                Box::pin(discord::gary::on_event(ctx, event, framework, event_data))
            },
            ..Default::default()
        })
        .setup(move |ctx, _ready, framework| {
            Box::pin(async move {
                poise::builtins::register_in_guild(ctx, &framework.options().commands, guild_id)
                    .await?;
                info!(
                    guild = guild_id.get(),
                    "registered guild-scoped slash commands"
                );
                Ok(data)
            })
        })
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

    spawn_shutdown_watch(std::sync::Arc::clone(&client.shard_manager));

    client
        .start()
        .await
        .context("discord gateway loop failed")?;
    Ok(())
}

/// Drain the gateway when SIGINT/SIGTERM arrives, in a background task.
fn spawn_shutdown_watch(shard_manager: std::sync::Arc<serenity::ShardManager>) {
    tokio::spawn(async move {
        wait_for_shutdown().await;
        info!("shutdown signal received, stopping discord client");
        shard_manager.shutdown_all().await;
    });
}

/// The guild-scoped slash commands the bot registers.
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
    spawn_backup_cycle(std::sync::Arc::clone(&service), handles);
    Some(service)
}

/// Snapshot every live server on the service's interval, in a background task.
fn spawn_backup_cycle(service: std::sync::Arc<backup::BackupService>, handles: CycleHandles) {
    let interval = service.interval();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick fires immediately; skip it so a restart doesn't snapshot
        // right away, only once per interval thereafter.
        ticker.tick().await;
        loop {
            ticker.tick().await;
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
