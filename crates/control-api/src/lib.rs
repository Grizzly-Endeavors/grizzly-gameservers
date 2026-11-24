//! Wire types shared between the in-pod supervisor and the Discord bot. Kept
//! dependency-light (serde only) so both sides agree on the contract without
//! pulling each other's transport stacks.
//!
//! Most of these describe the supervisor's control server, which the bot
//! consumes. Routing and bodies live together so a contract change is one edit
//! both sides recompile against: the lifecycle routes as [`ControlCommand`], the
//! body-carrying routes as path constants ([`FS_LIST_PATH`], [`FS_READ_PATH`],
//! [`FS_WRITE_PATH`], [`FS_EDIT_PATH`], [`FS_RESTORE_PATH`], [`LOGS_PATH`], [`COMMAND_PATH`],
//! [`ANNOUNCE_PATH`], plus [`ARCHIVE_PATH`] and [`OCCUPANCY_PATH`]), and each
//! route's request/response bodies beside them — so no route path is a bare
//! literal one side could rename into a silent 404. The control port both sides
//! bind and dial ([`CONTROL_PORT`]) lives here for the same reason — a port
//! mismatch is a silent drop, not an error. The reverse direction — the
//! supervisor posting in-game chat triggers to the bot's agent endpoint
//! ([`IngameTriggerRequest`] on [`INGAME_TRIGGER_PATH`]) — shares this crate too,
//! so the one contract file stays authoritative for both loops. The one contract here that is not a route at all is [`mods`] — the `resolved` file the in-pod reconciler writes and the bot reads back through the file API, which needs the same both-sides agreement for the same reason.

pub mod mods;
pub mod package;

use serde::{Deserialize, Serialize};

/// Suffix the supervisor passes to the Agones SDK `SetLabel` call to publish its
/// process state. Agones prefixes SDK-set labels with `agones.dev/sdk-`, so the
/// label lands on the `GameServer` as [`PROCESS_LABEL_KEY`].
pub const PROCESS_LABEL_SUFFIX: &str = "grizzly-process";

pub const PROCESS_LABEL_KEY: &str = "agones.dev/sdk-grizzly-process";

pub const PROCESS_LABEL_RUNNING: &str = "running";

pub const PROCESS_LABEL_STOPPED: &str = "stopped";

pub const CONTROL_PORT: u16 = 9359;

/// Default seconds the supervisor gives the game process to save its world after
/// SIGTERM before escalating to SIGKILL. A cross-component contract, which is why
/// it lives here rather than staying private to the supervisor: the game pod's
/// `terminationGracePeriodSeconds` must be at least this (or the kubelet SIGKILLs
/// the pod mid world-save), and the bot's supervisor HTTP client timeout must
/// exceed it (or the bot gives up on a `/stop` that is still legitimately running).
/// Generous relative to a Minecraft world-save. A game whose save runs longer
/// raises it per-image via `SUPERVISOR_GRACEFUL_TIMEOUT_SECS`.
///
/// Each `games/*/gameserver.yaml` hardcodes a `terminationGracePeriodSeconds` that
/// clears this value and can't import the const — a YAML file has no way to — so
/// that half is matched by hand, the same caveat [`CONTROL_PORT`] carries. The
/// drift guard in the bot's `tests/config.rs` reads the manifests off disk and
/// fails CI if one stops clearing the window.
pub const DEFAULT_GRACEFUL_TIMEOUT_SECS: u64 = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlCommand {
    Stop,
    Start,
    Restart,
    Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteError {
    NotFound,
    MethodNotAllowed,
}

impl ControlCommand {
    #[must_use]
    pub const fn method(self) -> &'static str {
        match self {
            Self::Stop | Self::Start | Self::Restart => "POST",
            Self::Status => "GET",
        }
    }

    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Stop => "/stop",
            Self::Start => "/start",
            Self::Restart => "/restart",
            Self::Status => "/status",
        }
    }

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessPhase {
    Starting,
    Running,
    Stopping,
    Stopped,
    Crashed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusResponse {
    pub process: ProcessPhase,
    pub ready: bool,
    pub pid: Option<u32>,
    pub uptime_seconds: u64,
    pub restarts: u32,
}

/// Body of `GET /occupancy`: how many players are currently connected, read live
/// over RCON. `players` is `null` when the count couldn't be determined — the
/// game has no RCON, or its console is unreachable/still starting. `None` always
/// means *unknown*, never a definite zero, so a caller must not read it as "empty".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OccupancyResponse {
    pub players: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultKind {
    Stopping,
    AlreadyStopped,
    Starting,
    AlreadyRunning,
    Restarting,
}

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

/// Query for the read-only filesystem routes (`GET /fs/list`, `GET /fs/read`).
/// `path` is relative to the supervisor's data root; the supervisor rejects any
/// value that escapes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathQuery {
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Dir,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub kind: EntryKind,
    #[serde(rename = "size")]
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListResponse {
    pub path: String,
    pub entries: Vec<DirEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadQuery {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_count: Option<usize>,
}

/// Body of `GET /fs/read`. `content` is UTF-8 (the supervisor refuses binary);
/// `truncated` is set when there was more of the file than came back — more
/// lines after the window, a single line cut at its character cap, or the byte
/// cap reached.
///
/// `start_line` and `total_lines` describe the window in lines. `total_lines` is
/// the whole file's count, not the window's, so a caller knows how much it
/// hasn't seen. Both are `0` for a whole-file read, which isn't addressed in
/// lines and doesn't count them.
///
/// `line_cut` is separate from `truncated` because the two call for different
/// responses: more file after the window is read by asking for the next window,
/// while a single line too long to show is only ever seen whole by searching for
/// it. Folded together they would be indistinguishable to the reader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadResponse {
    pub path: String,
    pub content: String,
    pub truncated: bool,
    pub line_cut: bool,
    pub start_line: usize,
    pub total_lines: usize,
}

/// Lines returned by `GET /fs/read` when the query names no window. Sized so an
/// ordinary game config comes back whole and a mod config does not, which is
/// what keeps `grep`-before-read worth doing on the files where it pays.
pub const DEFAULT_READ_LINES: usize = 400;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditRequest {
    pub path: String,
    pub old: String,
    pub new: String,
}

/// Body of the `POST /fs/edit` response.
///
/// `occurrences` is what `old` matched: exactly one is the success, and any
/// other value means nothing was written. Zero says the anchor isn't in the
/// file; more than one says it needs to be more specific, and carrying the count
/// lets the caller say which without a second read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditResponse {
    pub path: String,
    pub occurrences: usize,
    pub backed_up: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteRequest {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteResponse {
    pub path: String,
    pub backed_up: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreRequest {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreResponse {
    pub path: String,
}

/// Body of `POST /command`: an in-game console command to run over RCON against
/// the running server (e.g. `list`, `say hello`). The command is passed to the
/// game verbatim — no leading slash for Minecraft. Only served for games whose
/// per-game template enables RCON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandRequest {
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandResponse {
    pub output: String,
}

/// Body of `POST /shell`: one command line to run inside the game container.
///
/// Interpreted by `sh -c`, so pipelines and redirection parse as usual — but the
/// process is sandboxed read-only (see the supervisor's `shell` module), so a
/// redirection onto a game file is refused by the kernel rather than by parsing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellRequest {
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellResponse {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub truncated: bool,
    pub timed_out: bool,
}

/// Body of `POST /announce`: a message to broadcast to everyone on the running
/// server. The supervisor renders it with the game's own broadcast mechanism
/// (Minecraft `tellraw`), so the bot stays free of per-game console syntax. Only
/// served for games whose per-game template enables RCON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnounceRequest {
    pub message: String,
}

pub const FS_LIST_PATH: &str = "/fs/list";

pub const FS_READ_PATH: &str = "/fs/read";

pub const FS_WRITE_PATH: &str = "/fs/write";

pub const FS_EDIT_PATH: &str = "/fs/edit";

pub const FS_RESTORE_PATH: &str = "/fs/restore";

pub const LOGS_PATH: &str = "/logs";

pub const SHELL_PATH: &str = "/shell";

pub const COMMAND_PATH: &str = "/command";

pub const ANNOUNCE_PATH: &str = "/announce";

pub const ARCHIVE_PATH: &str = "/archive";

/// `GET /occupancy` reports the live connected-player count ([`OccupancyResponse`]).
/// Handled directly in the supervisor's control layer (an RCON read), independent
/// of the process-lifecycle state machine that answers `/status`. Shared so the
/// bot's client and the supervisor's server build and match the same path.
pub const OCCUPANCY_PATH: &str = "/occupancy";

/// Query for `GET /archive`. `quiesce` asks the supervisor to flush game state to
/// disk before the snapshot (e.g. Minecraft `save-off` + `save-all flush`, then
/// `save-on` after) so a *live* backup is internally consistent. Ignored when the
/// game has no RCON. Absent defaults to `false` — used by archive-then-teardown,
/// where the process is already stopped and there is nothing to flush.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveQuery {
    #[serde(default)]
    pub quiesce: bool,
}

/// Query for `POST /archive`. `purge` wipes the data root's contents before
/// extracting, so an overwrite-restore replaces the world rather than merging the
/// tar over whatever is already there. The purge is confined to the data root
/// (its contents, never the mount itself). Absent defaults to `false` — used when
/// seeding a freshly provisioned, empty PVC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractQuery {
    #[serde(default)]
    pub purge: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogsQuery {
    pub lines: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogsResponse {
    pub lines: Vec<String>,
}

pub const INGAME_TRIGGER_PATH: &str = "/ingame-trigger";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngameTriggerRequest {
    pub server: String,
    pub player: String,
    pub message: String,
}

#[cfg(test)]
#[path = "tests/lib.rs"]
mod tests;
