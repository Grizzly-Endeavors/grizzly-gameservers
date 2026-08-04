//! Argument handling for the supervisor binary.
//!
//! The supervisor takes all of its real configuration from the environment, so
//! the only argument it recognises is `--version`. That flag exists for the
//! image smoke check in `scripts/push-game-image.sh`: running it inside a freshly
//! built game image proves the binary actually loads and executes against that
//! vendor's runtime base before the image is pushed.

use std::ffi::OsStr;

/// Build identity printed by [`Invocation::ReportVersion`].
pub const VERSION: &str = concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION"));

const VERSION_FLAG: &str = "--version";

/// What the process should do with the arguments it was invoked with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invocation {
    /// Print [`VERSION`] and exit successfully.
    ReportVersion,
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
    if args
        .into_iter()
        .any(|arg| arg.as_ref() == OsStr::new(VERSION_FLAG))
    {
        Invocation::ReportVersion
    } else {
        Invocation::Supervise
    }
}

#[cfg(test)]
#[path = "tests/cli.rs"]
mod tests;
