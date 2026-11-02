//! `gary-local`: one Gary turn, or one of his tools, against game containers
//! started with plain `docker run` — no Discord, no Kubernetes, no Postgres.
//!
//! One JSON request arrives on stdin and one JSON response leaves on stdout, in
//! exactly the body shapes `POST /operator/gary` and `POST /operator/tool`
//! return, so an eval harness that drives the deployed bot drives this too.
//!
//! The servers are named in the request, each with the control URL its
//! supervisor answers on, and become a fixed [`ServerDirectory`]. Everything
//! that would reach the cluster, a database, an object store, the deferred-task
//! queue or a Discord channel is either absent from the [`Data`] built here or
//! disabled, and the tools that need any of it are not offered.

use std::io::Read as _;
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow, bail};
use poise::serenity_prelude as serenity;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::agent::{
    ChatMessage, SessionEvent, SessionStore, ToolCall, TurnActor, TurnIdentity, TurnOrigin,
};
use crate::agones::{FixedServer, FixedServers, ServerDirectory, ServerScope};
use crate::config::BotConfig;
use crate::discord::gary::tools::{
    ConfirmMode, ToolCtx, dispatch, known_tool, needs_only_supervisor, supervisor_only_tools,
};
use crate::discord::gary::{
    build_system_prompt_with_memory, drive_turn, framed_turn, game_catalog_list,
};
use crate::discord::{AccessLevel, Data, GatewayHandle};
use crate::http::operator::{
    GaryResponse, ModelProfile, OPERATOR_SPEAKER, ToolResponse, parse_access, resolve_connection,
};

/// Stand-ins for the Discord ids a turn is attributed to. Nothing here posts to
/// Discord or records an event, so they only have to be valid snowflakes.
const LOCAL_CHANNEL: u64 = 1;
const LOCAL_USER: u64 = 1;

/// Never dialled: every path that would reach the cluster is closed off locally,
/// and an address that refuses at once makes any one that slipped through fail
/// loudly instead of hanging.
const UNREACHABLE_CLUSTER: &str = "http://127.0.0.1:1";

/// The in-repo catalog, used when `GAMESERVERS_CATALOG_DIR` is unset — the
/// compiled default is the path inside the bot image.
const REPO_CATALOG_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../games");

/// Synthetic id for a tool request's call, as on the operator route.
const LOCAL_CALL_ID: &str = "local";

/// One request, as it arrives on stdin.
#[derive(Deserialize)]
struct LocalRequest {
    servers: Vec<ServerEntry>,
    #[serde(default)]
    access: Option<String>,
    /// Present for a turn: what to say to Gary, framed exactly as the operator
    /// route frames it.
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    model_profile: Option<ModelProfile>,
    /// Memory is never available locally; `true` is refused rather than ignored.
    #[serde(default)]
    memory: bool,
    /// Present for a tool request: the tool to run.
    #[serde(default)]
    tool: Option<String>,
    #[serde(default)]
    arguments: serde_json::Value,
}

#[derive(Deserialize)]
struct ServerEntry {
    name: String,
    game: String,
    control_url: String,
}

/// What a request asks for, once it is known to ask for exactly one thing.
enum Job {
    Turn {
        prompt: String,
        model_profile: Option<ModelProfile>,
    },
    Tool {
        name: String,
        arguments: serde_json::Value,
    },
}

/// Read one request from stdin, run it, and write the response to stdout.
///
/// # Errors
///
/// Returns an error when the request can't be served at all: it isn't valid
/// JSON, names an unknown game or tool, has no model to run on, or the model
/// call itself fails. A turn that escalates, or a tool that answers with an
/// error, is a response, not an error.
pub async fn run_gary_local() -> Result<()> {
    load_env_file()?;
    let mut raw = String::new();
    std::io::stdin()
        .read_to_string(&mut raw)
        .context("failed to read the request from stdin")?;
    let request: LocalRequest =
        serde_json::from_str(&raw).context("the request on stdin isn't valid JSON")?;
    let access = parse_access(request.access.as_deref()).map_err(|err| anyhow!("{err}"))?;
    if request.memory {
        bail!("memory is unavailable in gary-local; send \"memory\": false or leave it out");
    }
    let job = job_of(
        request.prompt,
        request.tool,
        request.model_profile,
        request.arguments,
    )?;
    let fixed = fixed_servers(request.servers)?;

    let config = local_config()?;
    let tasks = TaskTracker::new();
    let shutdown = CancellationToken::new();
    let data = local_data(config, fixed, &tasks, shutdown.clone()).await?;

    let body = match job {
        Job::Turn {
            prompt,
            model_profile,
        } => {
            serde_json::to_string(&run_turn(&data, access, &prompt, model_profile.as_ref()).await?)
        }
        Job::Tool { name, arguments } => {
            serde_json::to_string(&run_tool(&data, access, name, &arguments).await)
        }
    }
    .context("failed to encode the response")?;
    println!("{body}");

    shutdown.cancel();
    tasks.close();
    tasks.wait().await;
    Ok(())
}

/// Load `GAMESERVERS_ENV_FILE` when it is set, on top of whatever `.env` the
/// binary already loaded.
fn load_env_file() -> Result<()> {
    let Some(path) = std::env::var_os("GAMESERVERS_ENV_FILE") else {
        return Ok(());
    };
    dotenvy::from_path(&path).with_context(|| {
        format!(
            "failed to load GAMESERVERS_ENV_FILE at {}",
            std::path::Path::new(&path).display()
        )
    })
}

/// Decide whether the request is a turn or a tool call, refusing one that is
/// both or neither.
fn job_of(
    prompt: Option<String>,
    tool: Option<String>,
    model_profile: Option<ModelProfile>,
    arguments: serde_json::Value,
) -> Result<Job> {
    match (prompt, tool) {
        (Some(prompt), None) => {
            if prompt.trim().is_empty() {
                bail!("prompt is empty; send what you'd type to Gary");
            }
            Ok(Job::Turn {
                prompt,
                model_profile,
            })
        }
        (None, Some(name)) => {
            if !known_tool(&name) {
                bail!("unknown tool {name:?}");
            }
            if !needs_only_supervisor(&name) {
                bail!("tool {name:?} needs the cluster, so gary-local doesn't offer it");
            }
            Ok(Job::Tool { name, arguments })
        }
        (Some(_), Some(_)) => bail!("the request has both \"prompt\" and \"tool\"; send one"),
        (None, None) => bail!("the request has neither \"prompt\" nor \"tool\"; send one"),
    }
}

fn fixed_servers(entries: Vec<ServerEntry>) -> Result<FixedServers> {
    FixedServers::new(entries.into_iter().map(|entry| {
        (
            entry.name,
            FixedServer {
                game: entry.game,
                control_url: entry.control_url,
            },
        )
    }))
    .map_err(|reason| anyhow!("{reason}"))
}

/// The bot's configuration with every networked backing service removed, so no
/// credential in a loaded `.env` can make a local run reach one.
fn local_config() -> Result<BotConfig> {
    let lookup = |key: &str| {
        std::env::var_os(key).or_else(|| match key {
            // Required by the deployed bot; nothing here talks to Discord.
            "DISCORD_BOT_TOKEN" => Some("gary-local".into()),
            "GAMESERVERS_CATALOG_DIR" if std::path::Path::new(REPO_CATALOG_DIR).is_dir() => {
                Some(REPO_CATALOG_DIR.into())
            }
            _ => None,
        })
    };
    let mut config = BotConfig::from_env_with(&lookup)?;
    config.db = None;
    config.valkey = None;
    config.s3 = None;
    config.mail = None;
    config.langfuse = None;
    Ok(config)
}

/// The command state for a local run: the real catalog, settings, HTTP clients
/// and session machinery, a fixed server directory, and every durable or
/// networked subsystem in its disabled form.
async fn local_data(
    config: BotConfig,
    fixed: FixedServers,
    tasks: &TaskTracker,
    shutdown: CancellationToken,
) -> Result<Data> {
    // Validated before anything else so an unknown game is the error reported,
    // not whatever a half-built run trips over next.
    let catalog = crate::load_game_catalog(&config.catalog_dir).await?;
    if let Some(game) = fixed.games().find(|game| catalog.get(game).is_none()) {
        bail!(
            "unknown game {game:?}; the catalog at {} has: {}",
            config.catalog_dir.display(),
            catalog.game_ids().collect::<Vec<_>>().join(", ")
        );
    }

    let kube_client = kube::Client::try_from(kube::Config::new(
        UNREACHABLE_CLUSTER
            .parse()
            .context("failed to parse the placeholder cluster url")?,
    ))
    .context("failed to build the placeholder kubernetes client")?;
    let (http, control_http) = crate::build_http_clients(config.supervisor_token.as_deref())?;
    let telemetry = crate::setup_telemetry(&config, http.clone(), tasks, shutdown.clone());
    let settings = Arc::new(
        crate::settings::RuntimeSettings::connect(
            None,
            crate::settings::SettingsSnapshot::baseline(&config),
        )
        .await,
    );
    let effective = settings.snapshot();
    let durable = crate::DurableState::connect(None, Arc::clone(&settings)).await;
    let operator_ids: Arc<[u64]> = Arc::from(Vec::new());

    Ok(Data {
        kube_client,
        http,
        control_http,
        namespace: config.namespace.clone(),
        domain: config.domain.clone(),
        control_port: config.control_port,
        directory: ServerDirectory::Fixed(Arc::new(fixed)),
        catalog,
        provision_lock: Arc::new(tokio::sync::Mutex::new(())),
        notifier: crate::build_notifier(&config.token, &operator_ids),
        operator_ids,
        settings: Arc::clone(&settings),
        guild_config: durable.guild_config,
        ollama: crate::build_ollama(config.ollama_api_key.clone(), &effective),
        fireworks_api_key: config.fireworks_api_key.clone(),
        sessions: Arc::new(SessionStore::new()),
        home_channels: durable.home_channels,
        memory: durable.memory,
        sizing: durable.sizing,
        guild_limits: durable.guild_limits,
        backup: None,
        turn_throttle: Arc::new(crate::throttle::TurnThrottle::new(effective.throttle)),
        interrupts: Arc::new(crate::interrupt::TurnInterrupts::default()),
        tasks: tasks.clone(),
        defer: Arc::new(crate::defer::DeferRuntime::new(
            crate::kv::ValkeyClient::connect(None).await,
        )),
        shutdown,
        gateway: GatewayHandle::new(),
        environment: config.environment,
        telemetry,
        events: durable.events,
        signups: durable.signups,
        mailer: Arc::new(crate::mail::Mailer::connect(None)),
    })
}

/// Gary's tool context for a local run: admin-scoped to every listed server,
/// confirmations pre-approved as on the operator route, no Discord channel.
fn local_tool_ctx(data: &Data, access: AccessLevel) -> ToolCtx<'_> {
    ToolCtx {
        data,
        serenity: None,
        origin: TurnOrigin::Operator,
        channel_id: serenity::ChannelId::new(LOCAL_CHANNEL),
        guild: None,
        author_id: serenity::UserId::new(LOCAL_USER),
        access,
        scope: ServerScope::All,
        confirmations: ConfirmMode::Preapproved,
        pending_change: std::sync::Mutex::new(None),
        guardrail: std::sync::Mutex::new(None),
        escalation: std::sync::Mutex::new(None),
    }
}

/// Run one fresh turn on the system prompt the operator route builds with
/// memory off, offering only the tools a supervisor alone can serve.
async fn run_turn(
    data: &Data,
    access: AccessLevel,
    prompt: &str,
    model_profile: Option<&ModelProfile>,
) -> Result<GaryResponse> {
    let connection = resolve_connection(
        model_profile,
        data.ollama_now().as_ref(),
        data.fireworks_api_key.as_deref(),
    )
    .map_err(|err| anyhow!("{err}"))?
    .or_else(|| data.ollama_now())
    .ok_or_else(|| {
        anyhow!("no model to run the turn on: pass model_profile, or set OLLAMA_API_KEY")
    })?;

    let system = build_system_prompt_with_memory(access, &game_catalog_list(data), "", false);
    let mut messages = vec![
        ChatMessage::system(system),
        ChatMessage::user(framed_turn(OPERATOR_SPEAKER, access, prompt)),
    ];
    let mark = messages.len();
    let tool_defs = supervisor_only_tools(access);
    let tool_ctx = local_tool_ctx(data, access);
    let identity = TurnIdentity {
        origin: TurnOrigin::Operator,
        deployment: data.environment,
        guild: None,
        channel: None,
        actor: TurnActor::Discord(LOCAL_USER),
        access,
        session: None,
        game: None,
    };
    // Nobody is watching a local turn as it runs: the narration is already in
    // the transcript the response carries, and there is nothing to stop it.
    let progress = |_event: SessionEvent| {
        Box::pin(async {}) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
    };
    let cancel = CancellationToken::new();
    let outcome = drive_turn(
        &tool_ctx,
        identity,
        &connection,
        tool_defs.clone(),
        &mut messages,
        &progress,
        &cancel,
    )
    .await
    .context("the model call failed, so Gary couldn't answer")?;
    Ok(GaryResponse::from_turn(
        &outcome, &messages, mark, tool_defs,
    ))
}

/// Run one tool directly, through the same dispatcher a model-issued call takes.
/// Omitted arguments are sent as JSON `null`, as the operator route sends them.
async fn run_tool(
    data: &Data,
    access: AccessLevel,
    name: String,
    arguments: &serde_json::Value,
) -> ToolResponse {
    let call = ToolCall {
        id: LOCAL_CALL_ID.to_owned(),
        kind: "function".to_owned(),
        function: crate::agent::llm::FunctionCall {
            name: name.clone(),
            arguments: arguments.to_string(),
        },
    };
    let result = dispatch(&local_tool_ctx(data, access), &call).await;
    ToolResponse { tool: name, result }
}

#[cfg(test)]
#[path = "tests/local.rs"]
mod tests;
