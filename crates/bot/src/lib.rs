mod agent;
mod agones;
mod config;
mod discord;

pub use config::BotConfig;

use anyhow::{Context as _, Result};
use poise::serenity_prelude as serenity;
use tracing::{error, info, warn};

use agent::OllamaConfig;
use discord::{Data, commands};

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
    let guild_id = serenity::GuildId::new(config.guild_id);

    let ollama = config.ollama_api_key.map(|api_key| OllamaConfig {
        api_key,
        base_url: config.ollama_base_url,
        model: config.ollama_model,
    });
    if let Some(cfg) = &ollama {
        info!(model = %cfg.model, "agent (Gary) enabled");
    } else {
        warn!("OLLAMA_API_KEY not set; agent (Gary) disabled — mentions will be declined");
    }

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![
                commands::servers(),
                commands::create(),
                commands::kill(),
                commands::stop(),
                commands::start(),
                commands::restart(),
                commands::remove(),
            ],
            event_handler: |ctx, event, framework, data| {
                Box::pin(discord::gary::on_event(ctx, event, framework, data))
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
                Ok(Data {
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
                })
            })
        })
        .build();

    let intents = serenity::GatewayIntents::non_privileged();
    let mut client = serenity::ClientBuilder::new(config.token, intents)
        .framework(framework)
        .await
        .context("failed to build discord client")?;

    let shard_manager = std::sync::Arc::clone(&client.shard_manager);
    tokio::spawn(async move {
        wait_for_shutdown().await;
        info!("shutdown signal received, stopping discord client");
        shard_manager.shutdown_all().await;
    });

    client
        .start()
        .await
        .context("discord gateway loop failed")?;
    Ok(())
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
