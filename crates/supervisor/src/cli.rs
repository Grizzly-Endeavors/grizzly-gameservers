use std::ffi::OsStr;

use crate::shell::SANDBOX_ARG;

pub const VERSION: &str = concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION"));

const VERSION_FLAG: &str = "--version";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    ReportVersion,
    RunSandboxedShell(String),
    Supervise,
}

#[must_use]
pub fn classify_args<I, S>(args: I) -> Invocation
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut args = args.into_iter().peekable();
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
