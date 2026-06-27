use std::process::ExitCode;

use anyhow::Result;
use grizzly_gameservers::{BotConfig, run};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    load_dotenv();
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
