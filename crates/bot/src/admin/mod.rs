mod actions;
pub(crate) mod auth;
mod csrf;
mod pages;
mod render;

use axum::Router;
use axum::middleware::from_fn_with_state;
use axum::routing::get;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{error, info};

use crate::config::AdminConfig;
use crate::discord::Data;

use auth::AdminGate;

#[derive(Clone)]
pub(crate) struct AdminDeps {
    pub(crate) data: Data,
}

pub(crate) fn spawn(
    data: &Data,
    config: Option<AdminConfig>,
    tasks: &TaskTracker,
    shutdown: CancellationToken,
) {
    let Some(config) = config else {
        info!("no GAMESERVERS_ADMIN_PORT set; the admin panel is not served");
        return;
    };
    let deps = AdminDeps { data: data.clone() };
    tasks.spawn(async move {
        if let Err(err) = serve(deps, config, shutdown).await {
            error!(error = ?err, "admin panel listener failed");
        }
    });
}

async fn serve(
    deps: AdminDeps,
    config: AdminConfig,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let gate = AdminGate {
        required_group: config.required_group.clone(),
    };
    let origin = csrf::ExpectedOrigin {
        origin: config.public_origin.clone(),
    };
    let guarded = pages::router(deps)
        .layer(from_fn_with_state(origin, csrf::require_same_origin))
        .layer(from_fn_with_state(gate, auth::require_admin));
    let router = Router::new()
        .route("/panel.css", get(render::stylesheet))
        .merge(guarded);

    let address = std::net::SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, config.port));
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|err| anyhow::anyhow!("failed to bind the admin panel to {address}: {err}"))?;
    info!(
        port = config.port,
        required_group = %config.required_group,
        "admin panel listening"
    );
    axum::serve(listener, router)
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await
        .map_err(|err| anyhow::anyhow!("admin panel server error: {err}"))
}
