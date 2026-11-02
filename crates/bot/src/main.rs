use std::process::ExitCode;

use anyhow::Result;
use grizzly_gameservers::{BotConfig, run, run_gary_local};
use tracing_subscriber::EnvFilter;

/// The subcommand that runs one local Gary turn or tool call over stdin/stdout.
const GARY_LOCAL: &str = "gary-local";

#[tokio::main]
async fn main() -> ExitCode {
    load_dotenv();
    if std::env::args().nth(1).as_deref() == Some(GARY_LOCAL) {
        return gary_local().await;
    }
    init_tracing();

    match start().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!(error = ?err, "fatal error");
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn start() -> Result<()> {
    let config = BotConfig::from_env()?;
    run(config).await
}

/// `gary-local`: stdout carries only the response, so logs go to stderr, and a
/// setup failure is one line there with a non-zero exit.
async fn gary_local() -> ExitCode {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
    match run_gary_local().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// Load a local `.env` for development. A missing file is expected in-cluster
/// (env comes from the pod spec); any other parse error is surfaced as a
/// warning rather than swallowed.
fn load_dotenv() {
    if let Err(err) = dotenvy::dotenv()
        && !err.not_found()
    {
        eprintln!("warning: could not load .env file: {err}");
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
