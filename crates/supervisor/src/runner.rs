use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use grizzly_control_api::{
    ControlCommand, PROCESS_LABEL_RUNNING, PROCESS_LABEL_STOPPED, ProcessPhase, ResultKind,
};
use tokio::process::Child;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{Notify, mpsc};
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

use crate::autoupdate::{AutoUpdater, RelaunchReason};
use crate::config::SupervisorConfig;
use crate::control::{self, ControlReply, ControlRequest};
use crate::logs::LogBuffer;
use crate::rcon::RconRuntime;
use crate::readiness::LogReadyWatch;
use crate::sdk::SdkClient;
use crate::state::{ExitDisposition, SupervisorState};
use crate::{chat_watcher, palworld, process, readiness};

/// Brief pause before relaunching a crashed child, so a hard crash-loop backs
/// off rather than spinning the CPU until escalation.
const RELAUNCH_BACKOFF: Duration = Duration::from_secs(2);
const CONTROL_CHANNEL_DEPTH: usize = 16;
/// Timeout for SDK calls; loopback, so anything slower is a stuck sidecar.
const SDK_TIMEOUT: Duration = Duration::from_secs(5);
/// Buffered captured lines awaiting the chat watcher. Bounded so a stalled watcher
/// drops chat rather than growing without limit; sized well above normal chat rate.
const CHAT_CHANNEL_DEPTH: usize = 128;
/// Timeout for the trigger POST to the bot's agent endpoint. The reply returns
/// asynchronously over RCON, so this only bounds the handoff, not Gary's turn.
const CHAT_POST_TIMEOUT: Duration = Duration::from_secs(10);
/// Broadcast to players before a backstop auto-update relaunch (the rare case
/// where a server has stayed busy long enough that its build is past the hard age
/// cap). Friend-facing: plain language, no version numbers or internals.
const AUTO_UPDATE_WARNING: &str =
    "Heads up — restarting in a moment to apply a game update. You'll be able to rejoin shortly.";

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
            RconRuntime::new(port, cfg.rcon_dialect, cfg.rcon_password_max_len)
                .context("failed to initialize rcon client")?,
        )),
        None => None,
    };

    let chat_tx = spawn_chat_watcher(&cfg)?;

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

    supervise(cfg, sdk, control_rx, health_stop, logs, rcon, chat_tx).await
}

/// Ping the Agones SDK `/health` on a cadence — even while the game is paused —
/// so the pod is only torn down when the supervisor *decides* it should be
/// (crash escalation), not because a stopped game stopped answering.
async fn health_loop(sdk: SdkClient, interval: Duration, stop: Arc<Notify>) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Track health across ticks so a sustained outage warns once on the way down
    // and once on recovery, rather than every `interval` for as long as it lasts.
    let mut healthy = true;
    loop {
        tokio::select! {
            () = stop.notified() => {
                info!("stopping agones health heartbeat");
                return;
            }
            _ = ticker.tick() => {
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
    /// Tee for captured lines to the chat watcher, or `None` when the game hasn't
    /// enabled in-game chat. Threaded through relaunch so a bounced child's output
    /// keeps flowing to the watcher.
    chat_tx: Option<&'a mpsc::Sender<String>>,
}

async fn supervise(
    cfg: SupervisorConfig,
    sdk: SdkClient,
    mut control_rx: mpsc::Receiver<ControlRequest>,
    health_stop: Arc<Notify>,
    logs: Arc<LogBuffer>,
    rcon: Option<Arc<RconRuntime>>,
    chat_tx: Option<mpsc::Sender<String>>,
) -> Result<()> {
    // A cloneable handle for the occupancy poll's spawned queries; the borrowed
    // `rcon` below still drives the control paths.
    let rcon_poll = rcon.clone();
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
        let mut spawned = spawn_child(&cfg, rcon).context("failed to spawn initial child")?;
        let ready_watch = start_readiness(&cfg, &ready_tx);
        process::capture_output(&mut spawned, &logs, chat_tx.as_ref(), ready_watch.as_ref());
        let running = Some(spawned);
        note_started(&mut state, running.as_ref())?;
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
        chat_tx: chat_tx.as_ref(),
    };

    // Update-on-empty: poll occupancy on a cadence and, when the game has been
    // idle long enough on an old-enough build, bounce it to re-pull the latest.
    // Only meaningful when the game speaks RCON (needed for the player count).
    let auto_update_active = cfg.auto_update_enabled && rcon_poll.is_some();
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
                handle_unexpected_exit(&deps, &mut state, &mut child, &health_stop, exit).await;
            }
            Some(()) = ready_rx.recv() => {
                settle_ready(&sdk, &mut state).await;
            }
            _ = occupancy_ticker.tick(), if auto_update_active => {
                spawn_occupancy_poll(&state, rcon_poll.as_ref(), &occupancy_tx);
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

/// Start the in-game chat watcher when the game enables it, returning the sender
/// half the line pump tees captured output into. Returns `None` (no tee) when
/// chat watching is off, so the game's output path is untouched for games that
/// don't opt in.
///
/// # Errors
///
/// Returns an error if the watcher's HTTP client can't be built.
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

/// Record a freshly spawned child's pid/launch time on the state machine.
fn note_started(state: &mut SupervisorState, child: Option<&Child>) -> Result<()> {
    let pid = child
        .context("child slot empty after spawn")?
        .id()
        .context("spawned child has no pid")?;
    state.on_started(pid, Instant::now());
    Ok(())
}

/// Wire readiness for a freshly launched child. A game that declares a log
/// marker (`ready_log_pattern`) gets a [`LogReadyWatch`] the line pumps signal
/// through — needed for UDP-only games (Valheim) the TCP connect probe can never
/// reach. Every other game gets the background TCP connect probe on `game_port`.
/// Returns the watch to hand to `capture_output`, or `None` when the TCP probe is
/// in use.
fn start_readiness(cfg: &SupervisorConfig, ready_tx: &mpsc::Sender<()>) -> Option<LogReadyWatch> {
    if let Some(pattern) = cfg.ready_log_pattern.as_deref() {
        Some(LogReadyWatch::new(Arc::from(pattern), ready_tx.clone()))
    } else {
        spawn_readiness_probe(cfg.game_port, cfg.readiness_timeout, ready_tx.clone());
        None
    }
}

/// Probe for the game accepting connections in the background, signalling the
/// runner once it does. Re-spawned on every (re)launch.
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

/// Seed any config the supervisor owns into place, then launch the game child.
/// Today only Palworld needs seeding — its RCON keys written into the on-PVC ini
/// before the game reads it, since the image disables the upstream's destructive
/// env-driven regeneration (see [`palworld`]). Every other game passes straight
/// through to [`process::spawn`]. Runs before each launch so a fresh PVC and a
/// per-pod-rotated password both converge before the child starts.
///
/// # Errors
///
/// Returns an error if the config seed or the child spawn fails.
fn spawn_child(cfg: &SupervisorConfig, rcon: Option<&RconRuntime>) -> Result<Child> {
    if let (Some(rcon), Some(port), Some(ini)) =
        (rcon, cfg.rcon_port, cfg.palworld_ini_path.as_deref())
    {
        palworld::seed_rcon(ini, port, rcon.password())
            .context("failed to seed palworld rcon settings")?;
    }
    process::spawn(cfg, rcon)
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
    match spawn_child(deps.cfg, deps.rcon) {
        Ok(spawned) => {
            *child = Some(spawned);
            let ready_watch = start_readiness(deps.cfg, deps.ready_tx);
            if let Some(running) = child.as_mut() {
                process::capture_output(running, deps.logs, deps.chat_tx, ready_watch.as_ref());
            }
            if let Err(err) = note_started(state, child.as_ref()) {
                error!(error = ?err, action, "failed to record (re)launch");
            }
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

/// Fire off a one-shot occupancy query for the auto-updater, off the supervision
/// loop so a slow or wedged RCON console can't stall it. Only polls while the
/// game is actually up and accepting connections — a stopped or still-starting
/// console would just report `None` (unknown) anyway. The result flows back on
/// `occupancy_tx` for the loop's `maybe_auto_update` arm.
fn spawn_occupancy_poll(
    state: &SupervisorState,
    rcon: Option<&Arc<RconRuntime>>,
    occupancy_tx: &mpsc::Sender<Option<u32>>,
) {
    if state.status(Instant::now()).process != ProcessPhase::Running {
        return;
    }
    let Some(rcon) = rcon.cloned() else {
        return;
    };
    let tx = occupancy_tx.clone();
    tokio::spawn(async move {
        let players = rcon.player_count().await;
        if tx.send(players).await.is_err() {
            debug!("occupancy reading dropped; runner is gone");
        }
    });
}

/// Fold one occupancy reading into the auto-updater and, when it decides a build
/// is due for a refresh, bounce the game in place — the relaunch re-runs the
/// entrypoint, which re-pulls the latest server build. An idle bounce is silent
/// (nobody's connected); the rare backstop bounce warns players over RCON first.
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
    relaunch(deps, state, child, "auto-update").await;
    updater.note_relaunched();
}
