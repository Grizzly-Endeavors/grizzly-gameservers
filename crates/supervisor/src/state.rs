use std::time::{Duration, Instant};

use grizzly_control_api::{ProcessPhase, StatusResponse};

/// What the operator (via the control API) wants the child to be doing. The
/// runner drives the child toward this; the difference between desired and the
/// observed [`ProcessPhase`] is what an exit means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesiredState {
    Running,
    Stopped,
}

/// What the runner should do after observing an *unexpected* child exit (one it
/// did not itself initiate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitDisposition {
    /// The exit was expected (desired == Stopped); settle into Stopped.
    Clean,
    /// A crash within the budget; relaunch the child.
    Relaunch,
    /// Too many crashes in the window; stop the heartbeat and let Agones
    /// recreate the pod.
    Escalate,
}

/// The supervisor's view of the child process. Pure: every transition is a
/// method with no IO, so the runner stays a thin driver and the logic is tested
/// in isolation. Time is injected (`now: Instant`) rather than read, so crash-
/// window bookkeeping is deterministic under test.
#[derive(Debug, Clone)]
pub struct SupervisorState {
    desired: DesiredState,
    phase: ProcessPhase,
    pid: Option<u32>,
    readied: bool,
    restarts: u32,
    started_at: Option<Instant>,
    /// Timestamps of recent unexpected exits, pruned to `crash_window`.
    crash_times: Vec<Instant>,
}

impl Default for SupervisorState {
    fn default() -> Self {
        Self::new()
    }
}

impl SupervisorState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            desired: DesiredState::Running,
            phase: ProcessPhase::Starting,
            pid: None,
            readied: false,
            restarts: 0,
            started_at: None,
            crash_times: Vec::new(),
        }
    }

    #[must_use]
    pub fn desired(&self) -> DesiredState {
        self.desired
    }

    #[must_use]
    pub fn phase(&self) -> ProcessPhase {
        self.phase
    }

    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Record that a child process has been launched.
    pub fn on_started(&mut self, pid: u32, now: Instant) {
        self.pid = Some(pid);
        self.started_at = Some(now);
        // Keep `Running` if readiness was already signalled on an earlier boot
        // (a warm relaunch); otherwise we're still coming up.
        self.phase = if self.readied {
            ProcessPhase::Running
        } else {
            ProcessPhase::Starting
        };
    }

    /// Record that the child is accepting connections and the Agones SDK
    /// `/ready` has been signalled.
    pub fn on_ready(&mut self) {
        self.readied = true;
        if self.desired == DesiredState::Running {
            self.phase = ProcessPhase::Running;
        }
    }

    /// Operator asked to stop the game process (the pod stays up).
    pub fn on_stop_requested(&mut self) {
        self.desired = DesiredState::Stopped;
        self.phase = ProcessPhase::Stopping;
    }

    /// Operator asked to start the game process again.
    pub fn on_start_requested(&mut self) {
        self.desired = DesiredState::Running;
        self.phase = ProcessPhase::Starting;
    }

    /// Operator asked to bounce the process in place. Desired stays `Running`;
    /// the runner stops the child and relaunches it without routing the exit
    /// through the crash path.
    pub fn on_restart_requested(&mut self) {
        self.desired = DesiredState::Running;
        self.phase = ProcessPhase::Stopping;
    }

    /// Settle into the stopped state once the child has actually exited from an
    /// intentional stop.
    pub fn on_stopped(&mut self) {
        self.phase = ProcessPhase::Stopped;
        self.pid = None;
        self.started_at = None;
    }

    /// Classify an observed child exit and update phase/restart bookkeeping.
    ///
    /// When `desired == Stopped` the exit was expected ([`ExitDisposition::Clean`]).
    /// Otherwise it is a crash: it is counted within `crash_window`, and once
    /// `crash_threshold` crashes accumulate in that window the disposition
    /// becomes [`ExitDisposition::Escalate`]; below the threshold it is
    /// [`ExitDisposition::Relaunch`].
    pub fn on_child_exit(
        &mut self,
        now: Instant,
        crash_window: Duration,
        crash_threshold: u32,
    ) -> ExitDisposition {
        if self.desired == DesiredState::Stopped {
            self.on_stopped();
            return ExitDisposition::Clean;
        }

        self.pid = None;
        self.started_at = None;
        self.restarts = self.restarts.saturating_add(1);

        self.crash_times
            .retain(|&t| now.saturating_duration_since(t) <= crash_window);
        self.crash_times.push(now);

        let crashes_in_window = u32::try_from(self.crash_times.len()).unwrap_or(u32::MAX);
        if crashes_in_window >= crash_threshold {
            self.phase = ProcessPhase::Crashed;
            ExitDisposition::Escalate
        } else {
            self.phase = ProcessPhase::Starting;
            ExitDisposition::Relaunch
        }
    }

    /// Snapshot for the `GET /status` control route.
    #[must_use]
    pub fn status(&self, now: Instant) -> StatusResponse {
        let uptime_seconds = self
            .started_at
            .map_or(0, |t| now.saturating_duration_since(t).as_secs());
        StatusResponse {
            process: self.phase,
            ready: self.readied,
            pid: self.pid,
            uptime_seconds,
            restarts: self.restarts,
        }
    }
}

#[cfg(test)]
#[path = "tests/state.rs"]
mod tests;
