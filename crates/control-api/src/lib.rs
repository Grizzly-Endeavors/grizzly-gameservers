//! Wire types shared between the in-pod supervisor's HTTP control server and the
//! Discord bot's client. Kept dependency-light (serde only) so both sides agree
//! on the contract without pulling each other's transport stacks.
//!
//! The supervisor serves these; the bot consumes them. Routing ([`ControlCommand`])
//! and bodies ([`StatusResponse`], [`ControlOk`], [`ControlError`]) live together
//! so a contract change is one edit both sides recompile against.

use serde::{Deserialize, Serialize};

/// Suffix the supervisor passes to the Agones SDK `SetLabel` call to publish its
/// process state. Agones prefixes SDK-set labels with `agones.dev/sdk-`, so the
/// label lands on the `GameServer` as [`PROCESS_LABEL_KEY`].
pub const PROCESS_LABEL_SUFFIX: &str = "grizzly-process";

/// The full label key the bot reads off a `GameServer` to tell a paused server
/// (process down, pod up) from a running one. Kept beside the suffix the
/// supervisor writes so the two never drift.
pub const PROCESS_LABEL_KEY: &str = "agones.dev/sdk-grizzly-process";

/// Value of [`PROCESS_LABEL_KEY`] while the game process is running.
pub const PROCESS_LABEL_RUNNING: &str = "running";

/// Value of [`PROCESS_LABEL_KEY`] while the game process is intentionally stopped.
pub const PROCESS_LABEL_STOPPED: &str = "stopped";

/// A control action the bot can ask the supervisor to perform, identified by the
/// HTTP method + path it arrives on. Not a wire *body* — it is the routing key,
/// shared so the client builds the same paths the server matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlCommand {
    /// Gracefully stop the game process; keep the pod alive.
    Stop,
    /// Start the game process if it is stopped.
    Start,
    /// Bounce the game process in place.
    Restart,
    /// Report the current process phase.
    Status,
}

/// Why a request did not map to a [`ControlCommand`]. Maps to an HTTP status on
/// the server side (404 / 405).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteError {
    /// No route owns this path.
    NotFound,
    /// The path exists but not for this method.
    MethodNotAllowed,
}

impl ControlCommand {
    /// The HTTP method this command is issued with.
    #[must_use]
    pub const fn method(self) -> &'static str {
        match self {
            Self::Stop | Self::Start | Self::Restart => "POST",
            Self::Status => "GET",
        }
    }

    /// The request path this command is issued on.
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Stop => "/stop",
            Self::Start => "/start",
            Self::Restart => "/restart",
            Self::Status => "/status",
        }
    }

    /// Resolve a raw `(method, path)` to the command it addresses.
    ///
    /// # Errors
    ///
    /// Returns [`RouteError::NotFound`] when no route owns `path`, or
    /// [`RouteError::MethodNotAllowed`] when the path exists but not for `method`.
    pub fn from_request(method: &str, path: &str) -> Result<Self, RouteError> {
        let command = match path {
            "/stop" => Self::Stop,
            "/start" => Self::Start,
            "/restart" => Self::Restart,
            "/status" => Self::Status,
            _ => return Err(RouteError::NotFound),
        };
        if method == command.method() {
            Ok(command)
        } else {
            Err(RouteError::MethodNotAllowed)
        }
    }
}

/// The phase of the supervised game process, as reported by `GET /status` and
/// mirrored in the bot's listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessPhase {
    /// Launched, not yet accepting connections.
    Starting,
    /// Accepting connections.
    Running,
    /// A graceful stop is in flight.
    Stopping,
    /// Intentionally stopped; the pod (and supervisor) stay alive.
    Stopped,
    /// Exited unexpectedly and not (yet) relaunched.
    Crashed,
}

/// Body of `GET /status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusResponse {
    pub process: ProcessPhase,
    /// Whether the Agones SDK `/ready` has been signalled at least once.
    pub ready: bool,
    /// PID of the live child process, absent while stopped/crashed.
    pub pid: Option<u32>,
    /// Seconds since the current child was launched, `0` while stopped.
    pub uptime_seconds: u64,
    /// Count of unexpected exits the supervisor has relaunched from.
    pub restarts: u32,
}

/// The outcome of a control action, distinguishing a state change from a no-op
/// so the bot can phrase the friend-facing reply accurately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultKind {
    Stopping,
    AlreadyStopped,
    Starting,
    AlreadyRunning,
    Restarting,
}

/// Success body for the mutating control routes: `{"result": "..."}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlOk {
    pub result: ResultKind,
}

impl ControlOk {
    #[must_use]
    pub const fn new(result: ResultKind) -> Self {
        Self { result }
    }
}

/// Error body for any failed control route: `{"error": "..."}`. The message is
/// developer-facing; the bot translates outcomes into friend-facing copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlError {
    pub error: String,
}

impl ControlError {
    pub fn new(error: impl Into<String>) -> Self {
        Self {
            error: error.into(),
        }
    }
}

#[cfg(test)]
#[path = "tests/lib.rs"]
mod tests;
