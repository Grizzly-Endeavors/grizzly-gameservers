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

    pub(crate) fn is_enabled(&self) -> bool {
        self.signups.is_enabled()
    }
}

pub(crate) fn router(deps: SignupDeps, token: String) -> Router {
    gated(
        Router::new()
            .route(SIGNUP_PATH, post(submit))
            .with_state(deps),
        Some(token),
    )
}

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
        SubmitOutcome::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(SignupResponse {
                recorded: false,
                message: TRY_AGAIN,
            }),
        ),
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
