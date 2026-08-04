use std::process::ExitCode;

use anyhow::Result;
use grizzly_supervisor::cli::{self, Invocation};
use grizzly_supervisor::config::SupervisorConfig;
use grizzly_supervisor::runner;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    // Answered before anything reads the environment or opens a socket, so the
    // image smoke check can run it against a fully-configured game image.
    if cli::classify_args(std::env::args_os().skip(1)) == Invocation::ReportVersion {
        println!("{}", cli::VERSION);
        return ExitCode::SUCCESS;
    }

    load_dotenv();
    init_tracing();

    match start().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!(error = ?err, "supervisor exited with a fatal error");
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn start() -> Result<()> {
    let config = SupervisorConfig::from_env()?;
    runner::run(config).await
}

/// Load a local `.env` for development. A missing file is expected in-container
/// (env comes from the pod spec); any other parse error is surfaced as a warning
/// rather than swallowed.
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
