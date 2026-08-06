//! The signup intake route the public landing page posts to.
//!
//! The landing service is the internet-facing half of this path and holds no
//! database credential — it forwards a validated submission here, and this is
//! the only thing it can do to the signup table. That indirection is the point:
//! the public surface holds one bearer token whose entire capability is
//! "append a signup", and the schema, the queue, and every decision stay with
//! the process that owns them.
//!
//! Validation is deliberately repeated here rather than trusted from the
//! caller. The landing service validates so it can tell a person what they typed
//! wrong; this validates because a bearer token is not a promise of good input.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use super::gated;
use crate::signups::{
    MAX_NOTE_LEN, Signups, Submission, SubmitOutcome, normalize_email, truncate_chars,
};

/// Route path the landing service posts to. Named here rather than inline so the
/// landing repo's contract has one place to point at.
pub(crate) const SIGNUP_PATH: &str = "/signups";

#[derive(Clone)]
pub(crate) struct SignupDeps {
    signups: std::sync::Arc<Signups>,
}

impl SignupDeps {
    pub(crate) fn from_data(data: &crate::discord::Data) -> Self {
        Self {
            signups: std::sync::Arc::clone(&data.signups),
        }
    }

    /// Whether there is anywhere to record a submission. Read at the mount site,
    /// because a route that accepts a signup it can't store is worse than one
    /// that isn't there.
    pub(crate) fn is_enabled(&self) -> bool {
        self.signups.is_enabled()
    }
}

/// The intake route, gated on its own bearer token. The token is non-optional by
/// type: this route writes on behalf of an unauthenticated public form, so
/// "unconfigured" must mean unmounted rather than open — the same rule the
/// operator routes follow, and for the same reason.
pub(crate) fn router(deps: SignupDeps, token: String) -> Router {
    gated(
        Router::new()
            .route(SIGNUP_PATH, post(submit))
            .with_state(deps),
        Some(token),
    )
}

/// One form submission as the landing service forwards it.
#[derive(Debug, Deserialize)]
struct SignupRequest {
    email: String,
    #[serde(default)]
    wants_beta: bool,
    #[serde(default)]
    wants_updates: bool,
    #[serde(default)]
    note: Option<String>,
}

/// What the landing service tells the person who submitted the form.
///
/// Carries no hint about whether the address was already known: a public form
/// that answers differently for a registered address is an enumeration oracle,
/// and this endpoint is reachable by anyone who can reach that form.
#[derive(Debug, Serialize)]
struct SignupResponse {
    recorded: bool,
    message: &'static str,
}

const THANKS: &str = "You're on the list.";
const TRY_AGAIN: &str = "We couldn't save that just now. Please try again in a few minutes.";
const BAD_EMAIL: &str = "That doesn't look like an email address we can reach.";
const NOTHING_ASKED: &str = "Pick at least one thing to sign up for.";

async fn submit(
    State(deps): State<SignupDeps>,
    Json(request): Json<SignupRequest>,
) -> (StatusCode, Json<SignupResponse>) {
    let Some(email) = normalize_email(&request.email) else {
        return refuse(StatusCode::BAD_REQUEST, BAD_EMAIL);
    };
    // A submission that opts into nothing is a bug in the caller or a bot
    // filling every field it can see. Either way there is no signup to record.
    if !request.wants_beta && !request.wants_updates {
        return refuse(StatusCode::BAD_REQUEST, NOTHING_ASKED);
    }

    let note = request
        .note
        .map(|note| truncate_chars(note.trim(), MAX_NOTE_LEN))
        .filter(|note| !note.is_empty());
    let submission = Submission {
        email,
        display_email: request.email.trim().to_owned(),
        wants_beta: request.wants_beta,
        wants_updates: request.wants_updates,
        note,
    };

    match deps.signups.submit(&submission).await {
        SubmitOutcome::Recorded => {
            // The address itself is deliberately absent: this line lands in the
            // pod log, which is a wider audience than the panel behind Authentik.
            info!(
                beta = submission.wants_beta,
                updates = submission.wants_updates,
                "recorded a signup"
            );
            (
                StatusCode::OK,
                Json(SignupResponse {
                    recorded: true,
                    message: THANKS,
                }),
            )
        }
        SubmitOutcome::Unavailable => {
            // 503 rather than 500: the landing service should tell the person to
            // come back, and a retry genuinely may work.
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(SignupResponse {
                    recorded: false,
                    message: TRY_AGAIN,
                }),
            )
        }
    }
}

fn refuse(status: StatusCode, message: &'static str) -> (StatusCode, Json<SignupResponse>) {
    warn!(message, "refused a signup submission");
    (
        status,
        Json(SignupResponse {
            recorded: false,
            message,
        }),
    )
}

#[cfg(test)]
#[path = "tests/signup.rs"]
mod tests;
