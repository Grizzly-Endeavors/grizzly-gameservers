use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use grizzly_control_api::{
    ControlCommand, PROCESS_LABEL_RUNNING, PROCESS_LABEL_STOPPED, ProcessPhase, ResultKind,
};
use tokio::process::Child;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

use crate::auth::ControlAuth;
use crate::autoupdate::{AutoUpdater, RelaunchReason};
use crate::config::SupervisorConfig;
use crate::control::{self, ControlReply, ControlRequest};
use crate::heartbeat::Heartbeat;
use crate::launch_log::{self, LaunchLog};
use crate::logs::LogBuffer;
use crate::occupancy::{A2sQuery, OccupancyEndpoint, OccupancySource, SatisfactoryApi, TshockApi};
use crate::rcon::RconRuntime;
use crate::readiness::LogReadyWatch;
use crate::sdk::SdkClient;
use crate::state::{ExitDisposition, SupervisorState};
use crate::{chat_watcher, child_env, palworld, process, readiness, thunderstore};

const RELAUNCH_BACKOFF: Duration = Duration::from_secs(2);
const CONTROL_CHANNEL_DEPTH: usize = 16;
const SDK_TIMEOUT: Duration = Duration::from_secs(5);
const CHAT_CHANNEL_DEPTH: usize = 128;
const CHAT_POST_TIMEOUT: Duration = Duration::from_secs(10);
const AUTO_UPDATE_WARNING: &str =
    "Heads up — restarting in a moment to apply a game update. You'll be able to rejoin shortly.";

pub async fn run(cfg: SupervisorConfig) -> Result<()> {
    let http = reqwest::Client::builder()
        .timeout(SDK_TIMEOUT)
        .build()
        .context("failed to build agones SDK http client")?;
    let sdk = SdkClient::new(http, cfg.sdk_base_url.clone());

    let rcon = match cfg.rcon_port {
        Some(port) => Some(Arc::new(
            RconRuntime::new(port, cfg.rcon_dialect, cfg.rcon_password_max_len)
                .context("failed to initialize rcon client")?,
        )),
        None => None,
    };

    let occupancy = match (cfg.occupancy.as_ref(), rcon.as_ref()) {
        (Some(OccupancyEndpoint::Satisfactory(url)), _) => {
            Some(Arc::new(OccupancySource::SatisfactoryApi(Arc::new(
                SatisfactoryApi::new(url.clone())
                    .context("failed to initialize the satisfactory occupancy client")?,
            ))))
        }
        (Some(OccupancyEndpoint::Tshock(url)), _) => {
            Some(Arc::new(OccupancySource::TshockApi(Arc::new(
                TshockApi::new(url).context("failed to initialize the tshock occupancy client")?,
            ))))
        }
        (Some(OccupancyEndpoint::A2s(port)), _) => Some(Arc::new(OccupancySource::A2s(Arc::new(
            A2sQuery::new(*port),
        )))),
        (None, Some(rcon)) => Some(Arc::new(OccupancySource::Rcon(Arc::clone(rcon)))),
        (None, None) => None,
    };

    let chat_tx = spawn_chat_watcher(&cfg)?;

    if let Some(overlay) = cfg.child_env.as_ref()
        && let Err(err) = child_env::seed(&cfg.data_dir, &overlay.file)
    {
        warn!(error = ?err, file = %overlay.file, "could not seed the per-instance env overlay at startup");
    }

    let (control_tx, control_rx) = mpsc::channel::<ControlRequest>(CONTROL_CHANNEL_DEPTH);
    let logs = Arc::new(LogBuffer::new());
    let launch = Arc::new(LaunchLog::new());
    let control_port = cfg.control_port;
    let data_root: Arc<Path> = Arc::from(cfg.data_dir.clone());
    let serve_logs = Arc::clone(&logs);
    let serve_rcon = rcon.clone();
    let serve_occupancy = occupancy.clone();
    let control_auth = ControlAuth::new(cfg.control_token.as_deref());
    tokio::spawn(async move {
        if let Err(err) = control::serve(
            control_port,
            control_tx,
            data_root,
            serve_logs,
            serve_rcon,
            serve_occupancy,
            control_auth,
        )
        .await
        {
            error!(error = ?err, "control api terminated");
        }
    });

    let heartbeat = Heartbeat::new();
    tokio::spawn(health_loop(
        sdk.clone(),
        cfg.health_interval,
        heartbeat.clone(),
    ));

    supervise(
        cfg,
        sdk,
        control_rx,
        heartbeat,
        logs,
        launch,
        GameChannels {
            rcon,
            occupancy,
            chat_tx,
        },
    )
    .await
}

async fn health_loop(sdk: SdkClient, interval: Duration, heartbeat: Heartbeat) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut healthy = true;
    loop {
        ticker.tick().await;
        if !heartbeat.is_beating() {
            continue;
        }
        match sdk.health().await {
            Ok(()) => {
                if !healthy {
                    info!("agones health ping recovered");
                    healthy = true;
                }
            }
            Err(err) => {
                if healthy {
                    warn!(error = ?err, "agones health ping failing; the sdk sidecar may be unreachable");
                    healthy = false;
                } else {
                    debug!(error = ?err, "agones health ping still failing");
                }
            }
        }
    }
}

struct RunDeps<'a> {
    cfg: &'a SupervisorConfig,
    sdk: &'a SdkClient,
    ready_tx: &'a mpsc::Sender<()>,
    logs: &'a Arc<LogBuffer>,
    launch: &'a Arc<LaunchLog>,
    rcon: Option<&'a RconRuntime>,
    chat_tx: Option<&'a mpsc::Sender<String>>,
}

struct GameChannels {
    rcon: Option<Arc<RconRuntime>>,
    occupancy: Option<Arc<OccupancySource>>,
    chat_tx: Option<mpsc::Sender<String>>,
}

async fn supervise(
    cfg: SupervisorConfig,
    sdk: SdkClient,
    mut control_rx: mpsc::Receiver<ControlRequest>,
    heartbeat: Heartbeat,
    logs: Arc<LogBuffer>,
    launch: Arc<LaunchLog>,
    channels: GameChannels,
) -> Result<()> {
    let GameChannels {
        rcon,
        occupancy,
        chat_tx,
    } = channels;
    let rcon = rcon.as_deref();
    let mut state = SupervisorState::new();
    let (ready_tx, mut ready_rx) = mpsc::channel::<()>(1);
    let mut child = if cfg.start_paused {
        info!("starting paused; the game process will not launch until /start");
        state.on_stop_requested();
        state.on_stopped();
        publish_label(&sdk, PROCESS_LABEL_STOPPED).await;
        None
    } else {
        match spawn_child(&cfg, rcon, &logs).await {
            Ok(mut spawned) => {
                let ready_watch = start_readiness(&cfg, &ready_tx);
                launch.begin();
                process::capture_output(
                    &mut spawned,
                    &logs,
                    &launch,
                    chat_tx.as_ref(),
                    ready_watch.as_ref(),
                );
                let running = Some(spawned);
                note_started(&mut state, running.as_ref())?;
                running
            }
            Err(LaunchError::Prepare(err)) => {
                note_launch_failure(&mut state, &logs, &launch, &err);
                preserve_launch_output(&cfg.data_dir, &launch).await;
                publish_label(&sdk, PROCESS_LABEL_STOPPED).await;
                None
            }
            Err(LaunchError::Spawn(err)) => {
                return Err(err).context("failed to spawn initial child");
            }
        }
    };

    let mut sigterm =
        signal(SignalKind::terminate()).context("failed to install SIGTERM handler")?;
    let mut sigint = signal(SignalKind::interrupt()).context("failed to install SIGINT handler")?;

    info!(control_port = cfg.control_port, "supervisor running");

    let deps = RunDeps {
        cfg: &cfg,
        sdk: &sdk,
        ready_tx: &ready_tx,
        logs: &logs,
        launch: &launch,
        rcon,
        chat_tx: chat_tx.as_ref(),
    };

    let auto_update_active = cfg.auto_update_enabled && occupancy.is_some();
    let mut auto_updater = AutoUpdater::new(cfg.auto_update_policy);
    let (occupancy_tx, mut occupancy_rx) = mpsc::channel::<Option<u32>>(1);
    let mut occupancy_ticker = tokio::time::interval(cfg.auto_update_poll_interval);
    occupancy_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            Some(request) = control_rx.recv() => {
                handle_request(&deps, &mut state, &mut child, request).await;
            }
            exit = process::wait_optional(&mut child) => {
                handle_unexpected_exit(&deps, &mut state, &mut child, &heartbeat, exit).await;
            }
            Some(()) = ready_rx.recv() => {
                settle_ready(&deps, &mut state, &heartbeat).await;
            }
            _ = occupancy_ticker.tick(), if auto_update_active => {
                spawn_occupancy_poll(&state, occupancy.as_ref(), &occupancy_tx);
            }
            Some(players) = occupancy_rx.recv() => {
                maybe_auto_update(&deps, &mut state, &mut child, &mut auto_updater, players).await;
            }
            _ = sigterm.recv() => {
                info!("received SIGTERM; draining");
                break;
            }
            _ = sigint.recv() => {
                info!("received SIGINT; draining");
                break;
            }
        }
    }

    if let Some(mut running) = child.take()
        && let Err(err) = process::graceful_stop(&mut running, cfg.graceful_timeout).await
    {
        warn!(error = ?err, "graceful stop during shutdown failed");
    }
    Ok(())
}

fn spawn_chat_watcher(cfg: &SupervisorConfig) -> Result<Option<mpsc::Sender<String>>> {
    let Some(watch) = cfg.chat_watch.clone() else {
        return Ok(None);
    };
    let http = reqwest::Client::builder()
        .timeout(CHAT_POST_TIMEOUT)
        .build()
        .context("failed to build chat watcher http client")?;
    let (chat_tx, chat_rx) = mpsc::channel::<String>(CHAT_CHANNEL_DEPTH);
    info!(
        server = %watch.server,
        trigger = %watch.trigger,
        "in-game chat watcher enabled"
    );
    tokio::spawn(chat_watcher::run(chat_rx, watch, http));
    Ok(Some(chat_tx))
}

fn note_started(state: &mut SupervisorState, child: Option<&Child>) -> Result<()> {
    let pid = child
        .context("child slot empty after spawn")?
        .id()
        .context("spawned child has no pid")?;
    state.on_started(pid, Instant::now());
    Ok(())
}

fn start_readiness(cfg: &SupervisorConfig, ready_tx: &mpsc::Sender<()>) -> Option<LogReadyWatch> {
    if let Some(pattern) = cfg.ready_log_pattern.as_deref() {
        Some(LogReadyWatch::new(Arc::from(pattern), ready_tx.clone()))
    } else {
        spawn_readiness_probe(cfg.game_port, cfg.readiness_timeout, ready_tx.clone());
        None
    }
}

fn spawn_readiness_probe(port: u16, give_up: Duration, ready_tx: mpsc::Sender<()>) {
    tokio::spawn(async move {
        if readiness::wait_accepting(port, give_up).await {
            if ready_tx.send(()).await.is_err() {
                debug!("readiness signal dropped; runner is gone");
            }
        } else {
            warn!(
                port,
                "game did not accept connections before readiness timeout"
            );
        }
    });
}

async fn settle_ready(deps: &RunDeps<'_>, state: &mut SupervisorState, heartbeat: &Heartbeat) {
    let first_ready = !state.is_ready();
    state.on_ready();
    if !heartbeat.is_beating() {
        heartbeat.resume();
        state.clear_crash_history();
        info!("game recovered after crash escalation; resuming agones health heartbeat");
    }
    if first_ready && let Err(err) = deps.sdk.ready().await {
        warn!(error = ?err, "agones ready signal failed");
    }
    publish_label(deps.sdk, PROCESS_LABEL_RUNNING).await;
    spawn_orphaned_config_sweep(deps);
}

fn spawn_orphaned_config_sweep(deps: &RunDeps<'_>) {
    let planned = match thunderstore::OrphanedConfigSweep::plan(deps.cfg) {
        Ok(planned) => planned,
        Err(err) => {
            warn!(error = ?err, "cannot sweep the settings of uninstalled mods");
            return;
        }
    };
    let Some(sweep) = planned else {
        return;
    };
    let logs = Arc::clone(deps.logs);
    tokio::task::spawn_blocking(move || sweep.run(&logs));
}

async fn publish_label(sdk: &SdkClient, value: &str) {
    if let Err(err) = sdk.set_process_label(value).await {
        warn!(error = ?err, value, "failed to publish process-state label");
    }
}

async fn handle_request(
    deps: &RunDeps<'_>,
    state: &mut SupervisorState,
    child: &mut Option<Child>,
    request: ControlRequest,
) {
    let ControlRequest { command, reply } = request;
    let response = match command {
        ControlCommand::Stop => do_stop(deps, state, child).await,
        ControlCommand::Start => do_start(deps, state, child).await,
        ControlCommand::Restart => do_restart(deps, state, child).await,
        ControlCommand::Status => ControlReply::Status(state.status(Instant::now())),
    };
    if reply.send(response).is_err() {
        warn!("control client went away before the reply was sent");
    }
}

async fn do_stop(
    deps: &RunDeps<'_>,
    state: &mut SupervisorState,
    child: &mut Option<Child>,
) -> ControlReply {
    let Some(mut running) = child.take() else {
        return ControlReply::Acted(ResultKind::AlreadyStopped);
    };
    state.on_stop_requested();
    let stop_result = process::graceful_stop(&mut running, deps.cfg.graceful_timeout).await;
    state.on_stopped();
    publish_label(deps.sdk, PROCESS_LABEL_STOPPED).await;
    if let Err(err) = stop_result {
        error!(error = ?err, "failed to stop child cleanly");
        return ControlReply::Failed("failed to stop the server cleanly".to_owned());
    }
    ControlReply::Acted(ResultKind::Stopping)
}

async fn do_start(
    deps: &RunDeps<'_>,
    state: &mut SupervisorState,
    child: &mut Option<Child>,
) -> ControlReply {
    if child.is_some() {
        return ControlReply::Acted(ResultKind::AlreadyRunning);
    }
    state.on_start_requested();
    match relaunch(deps, state, child, "start").await {
        Launched::Running | Launched::Refused => ControlReply::Acted(ResultKind::Starting),
        Launched::Failed => ControlReply::Failed("failed to start the server".to_owned()),
    }
}

async fn do_restart(
    deps: &RunDeps<'_>,
    state: &mut SupervisorState,
    child: &mut Option<Child>,
) -> ControlReply {
    state.on_restart_requested();
    if let Some(mut running) = child.take()
        && let Err(err) = process::graceful_stop(&mut running, deps.cfg.graceful_timeout).await
    {
        error!(error = ?err, "failed to stop child during restart");
    }
    match relaunch(deps, state, child, "restart").await {
        Launched::Running | Launched::Refused => ControlReply::Acted(ResultKind::Restarting),
        Launched::Failed => ControlReply::Failed("failed to restart the server".to_owned()),
    }
}

enum LaunchError {
    Prepare(anyhow::Error),
    Spawn(anyhow::Error),
}

async fn spawn_child(
    cfg: &SupervisorConfig,
    rcon: Option<&RconRuntime>,
    logs: &LogBuffer,
) -> Result<Child, LaunchError> {
    if let (Some(rcon), Some(port), Some(ini)) =
        (rcon, cfg.rcon_port, cfg.palworld_ini_path.as_deref())
    {
        palworld::seed_rcon(ini, port, rcon.password())
            .context("failed to seed palworld rcon settings")
            .map_err(LaunchError::Prepare)?;
    }
    thunderstore::prepare(cfg, logs)
        .await
        .map_err(LaunchError::Prepare)?;
    let overlay = child_env::prepare(cfg);
    process::spawn(cfg, rcon, &overlay).map_err(LaunchError::Spawn)
}

fn note_launch_failure(
    state: &mut SupervisorState,
    logs: &LogBuffer,
    launch: &LaunchLog,
    err: &anyhow::Error,
) {
    error!(error = ?err, "launch preparation failed; reporting the server as crashed");
    let line = format!("[mods] failed: {err:#}");
    launch.push(line.clone());
    logs.push(line);
    state.on_launch_failed();
}

async fn relaunch(
    deps: &RunDeps<'_>,
    state: &mut SupervisorState,
    child: &mut Option<Child>,
    action: &str,
) -> Launched {
    deps.launch.begin();
    match spawn_child(deps.cfg, deps.rcon, deps.logs).await {
        Ok(spawned) => {
            *child = Some(spawned);
            let ready_watch = start_readiness(deps.cfg, deps.ready_tx);
            if let Some(running) = child.as_mut() {
                process::capture_output(
                    running,
                    deps.logs,
                    deps.launch,
                    deps.chat_tx,
                    ready_watch.as_ref(),
                );
            }
            if let Err(err) = note_started(state, child.as_ref()) {
                error!(error = ?err, action, "failed to record (re)launch");
            }
            publish_label(deps.sdk, PROCESS_LABEL_RUNNING).await;
            Launched::Running
        }
        Err(LaunchError::Prepare(err)) => {
            *child = None;
            note_launch_failure(state, deps.logs, deps.launch, &err);
            preserve_launch_output(&deps.cfg.data_dir, deps.launch).await;
            publish_label(deps.sdk, PROCESS_LABEL_STOPPED).await;
            Launched::Refused
        }
        Err(LaunchError::Spawn(err)) => {
            error!(error = ?err, action, "failed to (re)launch child");
            *child = None;
            Launched::Failed
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Launched {
    Running,
    Refused,
    Failed,
}

async fn preserve_launch_output(data_dir: &std::path::Path, launch: &LaunchLog) {
    if !launch.settle().await {
        warn!(
            "game output was still draining when the launch was preserved; its final lines may be missing"
        );
    }
    let Some(body) = launch.render() else {
        debug!("crashed launch produced no output to preserve");
        return;
    };
    match launch_log::preserve(data_dir, &body) {
        Ok(path) => info!(artifact = %path.display(), "preserved the crashed launch's output"),
        Err(err) => warn!(error = ?err, "could not preserve the crashed launch's output"),
    }
}

async fn handle_unexpected_exit(
    deps: &RunDeps<'_>,
    state: &mut SupervisorState,
    child: &mut Option<Child>,
    heartbeat: &Heartbeat,
    exit: std::io::Result<std::process::ExitStatus>,
) {
    match exit {
        Ok(status) => warn!(?status, "game process exited unexpectedly"),
        Err(err) => error!(error = ?err, "failed waiting on game process"),
    }
    *child = None;
    preserve_launch_output(&deps.cfg.data_dir, deps.launch).await;
    match state.on_child_exit(
        Instant::now(),
        deps.cfg.crash_window,
        deps.cfg.crash_threshold,
    ) {
        ExitDisposition::Clean => publish_label(deps.sdk, PROCESS_LABEL_STOPPED).await,
        ExitDisposition::Relaunch => {
            warn!("relaunching crashed game process after backoff");
            sleep(RELAUNCH_BACKOFF).await;
            let _outcome = relaunch(deps, state, child, "crash-relaunch").await;
        }
        ExitDisposition::Escalate => {
            error!(
                "crash threshold exceeded; stopping health heartbeat so agones recreates the pod \
                 unless the game is repaired and relaunched first"
            );
            heartbeat.suspend();
            publish_label(deps.sdk, PROCESS_LABEL_STOPPED).await;
        }
    }
}

fn spawn_occupancy_poll(
    state: &SupervisorState,
    occupancy: Option<&Arc<OccupancySource>>,
    occupancy_tx: &mpsc::Sender<Option<u32>>,
) {
    if state.status(Instant::now()).process != ProcessPhase::Running {
        return;
    }
    let Some(occupancy) = occupancy.cloned() else {
        return;
    };
    let tx = occupancy_tx.clone();
    tokio::spawn(async move {
        let players = occupancy.player_count().await;
        if tx.send(players).await.is_err() {
            debug!("occupancy reading dropped; runner is gone");
        }
    });
}

async fn maybe_auto_update(
    deps: &RunDeps<'_>,
    state: &mut SupervisorState,
    child: &mut Option<Child>,
    updater: &mut AutoUpdater,
    players: Option<u32>,
) {
    let now = Instant::now();
    let version_age = Duration::from_secs(state.status(now).uptime_seconds);
    let Some(reason) = updater.observe(players, version_age, now) else {
        return;
    };
    match reason {
        RelaunchReason::Idle => info!(
            version_age_secs = version_age.as_secs(),
            "auto-updating idle server to the latest build"
        ),
        RelaunchReason::Backstop => {
            warn!(
                version_age_secs = version_age.as_secs(),
                players = ?players,
                "auto-updating busy server: build past the max-age cap, warning players first"
            );
            if let Some(rcon) = deps.rcon
                && let Err(err) = rcon.broadcast(AUTO_UPDATE_WARNING).await
            {
                warn!(error = ?err, "failed to warn players before the auto-update relaunch");
            }
        }
    }
    state.on_restart_requested();
    if let Some(mut running) = child.take()
        && let Err(err) = process::graceful_stop(&mut running, deps.cfg.graceful_timeout).await
    {
        error!(error = ?err, "failed to stop child during auto-update relaunch");
    }
    let _outcome = relaunch(deps, state, child, "auto-update").await;
    updater.note_relaunched();
}
