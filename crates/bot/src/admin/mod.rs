//! The super-admin panel: a browser view of what the bot is doing and what it is
//! configured with.
//!
//! Distinct from Discord's admin tier in both audience and reach. A guild admin
//! runs *their* Discord server — they can hand out roles and destroy their own
//! game servers. This panel is the operator's, and it reaches the knobs that
//! bound every guild at once: who is admitted at all, what the caps are, which
//! model Gary drives. Those were deliberately kept out of Discord (ADR-022 put
//! the cap mutations behind `require_operator` for exactly this reason), which
//! left them reachable only through a redeploy. This is where they live now.
//!
//! **Its own listener, not a route on the agent endpoint.** Port 9360 has to stay
//! reachable by game pods for the in-game `@Gary` trigger, and a game pod runs a
//! third-party server binary. The panel trusts request headers to say who is
//! asking, so it must sit where only the ingress controller can reach it. Two
//! ports let one `CiliumNetworkPolicy` say exactly that. See
//! [ADR-025](../../docs/decisions/025-super-admin-panel.md), and note this is a
//! considered departure from ADR-011's "these routes stay ClusterIP-only".
//!
//! The listener does not bind at all unless `GAMESERVERS_ADMIN_PORT` is set —
//! fail-closed by absence, the same shape as the operator token, so a
//! half-configured deploy serves nothing rather than something unguarded.

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

/// What the panel's handlers work from. A whole `Data` clone, following the
/// operator harness' shape (`crate::http::operator`): every façade the pages
/// read is already an `Arc` inside it, so this is a handful of refcount bumps
/// rather than a parallel set of handles to keep in sync.
#[derive(Clone)]
pub(crate) struct AdminDeps {
    pub(crate) data: Data,
}

/// Start the panel's listener on the tracked task set, if it is configured.
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

/// Bind and serve until `shutdown` is cancelled.
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
    // Layers run outermost-first, so identity is established before the
    // same-origin check — a cross-origin write from a stranger should be logged
    // as unidentified, not as a bad origin.
    let guarded = pages::router(deps)
        .layer(from_fn_with_state(origin, csrf::require_same_origin))
        .layer(from_fn_with_state(gate, auth::require_admin));
    // The stylesheet sits outside the gate: it carries nothing, and a denial
    // page that can't load its own styling is a worse first impression than a
    // publicly readable CSS file is a risk.
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
