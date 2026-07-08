use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use grizzly_control_api::{
    ControlCommand, PROCESS_LABEL_RUNNING, PROCESS_LABEL_STOPPED, ResultKind,
};
use tokio::process::Child;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{Notify, mpsc};
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

use crate::config::SupervisorConfig;
use crate::control::{self, ControlReply, ControlRequest};
use crate::logs::LogBuffer;
use crate::rcon::RconRuntime;
use crate::sdk::SdkClient;
use crate::state::{ExitDisposition, SupervisorState};
use crate::{process, readiness};

/// How long the readiness probe waits for the game to bind before giving up.
/// Generous: first-boot Minecraft world generation can run for minutes.
const READINESS_GIVE_UP: Duration = Duration::from_mins(10);
/// Brief pause before relaunching a crashed child, so a hard crash-loop backs
/// off rather than spinning the CPU until escalation.
const RELAUNCH_BACKOFF: Duration = Duration::from_secs(2);
const CONTROL_CHANNEL_DEPTH: usize = 16;
/// Timeout for SDK calls; loopback, so anything slower is a stuck sidecar.
const SDK_TIMEOUT: Duration = Duration::from_secs(5);

/// Boot the control server and health heartbeat, then run the supervision loop
/// until a shutdown signal.
///
/// # Errors
///
/// Returns an error if the HTTP client or signal handlers can't be built or the
/// initial child fails to spawn.
pub async fn run(cfg: SupervisorConfig) -> Result<()> {
    let http = reqwest::Client::builder()
        .timeout(SDK_TIMEOUT)
        .build()
        .context("failed to build agones SDK http client")?;
    let sdk = SdkClient::new(http, cfg.sdk_base_url.clone());

    // Mint the RCON password once per pod (not per child) so it stays stable
    // across in-place restarts; `None` leaves the /command route disabled.
    let rcon = match cfg.rcon_port {
        Some(port) => Some(Arc::new(
            RconRuntime::new(port, cfg.rcon_minecraft)
                .context("failed to initialize rcon client")?,
        )),
        None => None,
    };

    let (control_tx, control_rx) = mpsc::channel::<ControlRequest>(CONTROL_CHANNEL_DEPTH);
    let logs = Arc::new(LogBuffer::new());
    let control_port = cfg.control_port;
    let data_root: Arc<Path> = Arc::from(cfg.data_dir.clone());
    let serve_logs = Arc::clone(&logs);
    let serve_rcon = rcon.clone();
    tokio::spawn(async move {
        if let Err(err) =
            control::serve(control_port, control_tx, data_root, serve_logs, serve_rcon).await
        {
            error!(error = ?err, "control api terminated");
        }
    });

    let health_stop = Arc::new(Notify::new());
    tokio::spawn(health_loop(
        sdk.clone(),
        cfg.health_interval,
        Arc::clone(&health_stop),
    ));

    supervise(cfg, sdk, control_rx, health_stop, logs, rcon).await
}

/// Ping the Agones SDK `/health` on a cadence — even while the game is paused —
/// so the pod is only torn down when the supervisor *decides* it should be
/// (crash escalation), not because a stopped game stopped answering.
async fn health_loop(sdk: SdkClient, interval: Duration, stop: Arc<Notify>) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = stop.notified() => {
                info!("stopping agones health heartbeat");
                return;
            }
            _ = ticker.tick() => {
                if let Err(err) = sdk.health().await {
                    warn!(error = ?err, "agones health ping failed");
                }
            }
        }
    }
}

/// The long-lived dependencies the supervision handlers share, bundled so each
/// handler takes one context argument instead of threading four through every
/// call (and tripping the argument-count lint).
struct RunDeps<'a> {
    cfg: &'a SupervisorConfig,
    sdk: &'a SdkClient,
    ready_tx: &'a mpsc::Sender<()>,
    logs: &'a Arc<LogBuffer>,
    rcon: Option<&'a RconRuntime>,
}

async fn supervise(
    cfg: SupervisorConfig,
    sdk: SdkClient,
    mut control_rx: mpsc::Receiver<ControlRequest>,
    health_stop: Arc<Notify>,
    logs: Arc<LogBuffer>,
    rcon: Option<Arc<RconRuntime>>,
) -> Result<()> {
    let rcon = rcon.as_deref();
    let mut state = SupervisorState::new();
    let (ready_tx, mut ready_rx) = mpsc::channel::<()>(1);
    let mut child = if cfg.start_paused {
        // Restore path: hold the game down so the bot can seed /data over the
        // control API before the first launch (otherwise the game would generate
        // a throwaway fresh world we'd immediately overwrite). Present as Stopped
        // so the bot's process-state label reads correctly until it issues /start.
        info!("starting paused; the game process will not launch until /start");
        state.on_stop_requested();
        state.on_stopped();
        publish_label(&sdk, PROCESS_LABEL_STOPPED).await;
        None
    } else {
        let mut spawned = process::spawn(&cfg, rcon).context("failed to spawn initial child")?;
        process::capture_output(&mut spawned, &logs);
        let running = Some(spawned);
        note_started(&mut state, running.as_ref())?;
        spawn_readiness_probe(cfg.game_port, ready_tx.clone());
        running
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
        rcon,
    };

    loop {
        tokio::select! {
            Some(request) = control_rx.recv() => {
                handle_request(&deps, &mut state, &mut child, request).await;
            }
            exit = process::wait_optional(&mut child) => {
                handle_unexpected_exit(&deps, &mut state, &mut child, &health_stop, exit).await;
            }
            Some(()) = ready_rx.recv() => {
                settle_ready(&sdk, &mut state).await;
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

/// Record a freshly spawned child's pid/launch time on the state machine.
fn note_started(state: &mut SupervisorState, child: Option<&Child>) -> Result<()> {
    let pid = child
        .context("child slot empty after spawn")?
        .id()
        .context("spawned child has no pid")?;
    state.on_started(pid, Instant::now());
    Ok(())
}

/// Probe for the game accepting connections in the background, signalling the
/// runner once it does. Re-spawned on every (re)launch.
fn spawn_readiness_probe(port: u16, ready_tx: mpsc::Sender<()>) {
    tokio::spawn(async move {
        if readiness::wait_accepting(port, READINESS_GIVE_UP).await {
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

/// First time the game is accepting, signal Agones `Ready`; always (re)assert the
/// running process label.
async fn settle_ready(sdk: &SdkClient, state: &mut SupervisorState) {
    let first_ready = !state.is_ready();
    state.on_ready();
    if first_ready && let Err(err) = sdk.ready().await {
        warn!(error = ?err, "agones ready signal failed");
    }
    publish_label(sdk, PROCESS_LABEL_RUNNING).await;
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

/// Stop the game process in place, reaping the child inline so the loop's
/// child-exit arm doesn't treat the intentional exit as a crash.
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
    relaunch(deps, state, child, "start").await;
    if child.is_some() {
        ControlReply::Acted(ResultKind::Starting)
    } else {
        ControlReply::Failed("failed to start the server".to_owned())
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
    relaunch(deps, state, child, "restart").await;
    if child.is_some() {
        ControlReply::Acted(ResultKind::Restarting)
    } else {
        ControlReply::Failed("failed to restart the server".to_owned())
    }
}

/// Spawn a fresh child, record it, kick off a readiness probe, and assert the
/// running label. `action` names the operation in logs. On spawn failure `child`
/// is left empty so the caller reports the failure.
async fn relaunch(
    deps: &RunDeps<'_>,
    state: &mut SupervisorState,
    child: &mut Option<Child>,
    action: &str,
) {
    match process::spawn(deps.cfg, deps.rcon) {
        Ok(spawned) => {
            *child = Some(spawned);
            if let Some(running) = child.as_mut() {
                process::capture_output(running, deps.logs);
            }
            if let Err(err) = note_started(state, child.as_ref()) {
                error!(error = ?err, action, "failed to record (re)launch");
            }
            spawn_readiness_probe(deps.cfg.game_port, deps.ready_tx.clone());
            publish_label(deps.sdk, PROCESS_LABEL_RUNNING).await;
        }
        Err(err) => {
            error!(error = ?err, action, "failed to (re)launch child");
            *child = None;
        }
    }
}

async fn handle_unexpected_exit(
    deps: &RunDeps<'_>,
    state: &mut SupervisorState,
    child: &mut Option<Child>,
    health_stop: &Arc<Notify>,
    exit: std::io::Result<std::process::ExitStatus>,
) {
    match exit {
        Ok(status) => warn!(?status, "game process exited unexpectedly"),
        Err(err) => error!(error = ?err, "failed waiting on game process"),
    }
    *child = None;
    match state.on_child_exit(
        Instant::now(),
        deps.cfg.crash_window,
        deps.cfg.crash_threshold,
    ) {
        // Intentional stops reap inline, so a Clean verdict here is defensive only.
        ExitDisposition::Clean => publish_label(deps.sdk, PROCESS_LABEL_STOPPED).await,
        ExitDisposition::Relaunch => {
            warn!("relaunching crashed game process after backoff");
            sleep(RELAUNCH_BACKOFF).await;
            relaunch(deps, state, child, "crash-relaunch").await;
        }
        ExitDisposition::Escalate => {
            error!(
                "crash threshold exceeded; stopping health heartbeat so agones recreates the pod"
            );
            health_stop.notify_one();
            publish_label(deps.sdk, PROCESS_LABEL_STOPPED).await;
        }
    }
}
