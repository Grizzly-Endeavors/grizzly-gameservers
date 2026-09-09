//! Argument handling for the supervisor binary.
//!
//! The supervisor takes all of its real configuration from the environment, so
//! it recognises only two arguments. `--version` exists for the image smoke
//! check in `scripts/push-game-image.sh`: running it inside a freshly built game
//! image proves the binary actually loads and executes against that vendor's
//! runtime base before the image is pushed. [`shell::SANDBOX_ARG`] is how the
//! process re-executes itself to enter the ops agent's read-only shell sandbox
//! (see [`crate::shell`]).

use std::ffi::OsStr;

use crate::shell::SANDBOX_ARG;

/// Build identity printed by [`Invocation::ReportVersion`].
pub const VERSION: &str = concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION"));

const VERSION_FLAG: &str = "--version";

/// What the process should do with the arguments it was invoked with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    /// Print [`VERSION`] and exit successfully.
    ReportVersion,
    /// Sandbox this process and become `sh -c` running the carried command —
    /// the ops agent's read-only shell, re-executed from [`crate::shell`].
    RunSandboxedShell(String),
    /// Supervise the game process — the normal container entrypoint path.
    Supervise,
}

/// Classify process arguments, which must already exclude `argv[0]`.
#[must_use]
pub fn classify_args<I, S>(args: I) -> Invocation
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut args = args.into_iter().peekable();
    // Positional and first, unlike --version, so a command containing the
    // literal string can never be mistaken for the marker that introduces it.
    if args
        .peek()
        .is_some_and(|arg| arg.as_ref() == OsStr::new(SANDBOX_ARG))
    {
        let command = args
            .nth(1)
            .and_then(|arg| arg.as_ref().to_str().map(str::to_owned))
            .unwrap_or_default();
        return Invocation::RunSandboxedShell(command);
    }
    if args.any(|arg| arg.as_ref() == OsStr::new(VERSION_FLAG)) {
        Invocation::ReportVersion
    } else {
        Invocation::Supervise
    }
}

#[cfg(test)]
#[path = "tests/cli.rs"]
mod tests;
