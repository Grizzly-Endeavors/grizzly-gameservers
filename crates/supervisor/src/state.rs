use std::time::{Duration, Instant};

use grizzly_control_api::{ProcessPhase, StatusResponse};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesiredState {
    Running,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitDisposition {
    Clean,
    Relaunch,
    Escalate,
}

#[derive(Debug, Clone)]
pub struct SupervisorState {
    desired: DesiredState,
    phase: ProcessPhase,
    pid: Option<u32>,
    readied: bool,
    restarts: u32,
    started_at: Option<Instant>,
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
    pub fn is_ready(&self) -> bool {
        self.readied
    }

    pub fn on_started(&mut self, pid: u32, now: Instant) {
        self.pid = Some(pid);
        self.started_at = Some(now);
        self.phase = ProcessPhase::Starting;
    }

    pub fn on_ready(&mut self) {
        self.readied = true;
        if self.desired == DesiredState::Running {
            self.phase = ProcessPhase::Running;
        }
    }

    pub fn on_stop_requested(&mut self) {
        self.desired = DesiredState::Stopped;
        self.phase = ProcessPhase::Stopping;
    }

    pub fn on_start_requested(&mut self) {
        self.desired = DesiredState::Running;
        self.phase = ProcessPhase::Starting;
    }

    pub fn on_restart_requested(&mut self) {
        self.desired = DesiredState::Running;
        self.phase = ProcessPhase::Stopping;
    }

    pub fn on_stopped(&mut self) {
        self.phase = ProcessPhase::Stopped;
        self.pid = None;
        self.started_at = None;
    }

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

    pub fn on_launch_failed(&mut self) {
        self.phase = ProcessPhase::Crashed;
        self.pid = None;
        self.started_at = None;
    }

    pub fn clear_crash_history(&mut self) {
        self.crash_times.clear();
    }

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
