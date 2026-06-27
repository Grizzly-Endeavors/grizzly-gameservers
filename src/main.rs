use std::process::ExitCode;

use anyhow::Result;
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    init_tracing();

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!(error = ?err, "fatal error");
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "scaffold returns Result so real logic can use ? without resignaturing"
)]
fn run() -> Result<()> {
    tracing::info!("grizzly-gameservers starting");
    println!("Hello from grizzly-gameservers!");
    Ok(())
}
