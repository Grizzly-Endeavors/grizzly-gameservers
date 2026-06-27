use poise::serenity_prelude as serenity;
use serenity::{Colour, CreateEmbed};

use crate::agones::{CreateOutcome, RemoveOutcome, ServerSummary, StartOutcome, StopOutcome};

const EMPTY_MESSAGE: &str = "No game servers are running right now.";
const NO_ADDRESS: &str = "(not exposed yet)";

// Outcome palette. Green = running/ready, amber = still coming up, red =
// failure or destructive, slate = neutral/no-op so a plain "ok, nothing to do"
// doesn't read as alarming.
const COLOUR_UP: Colour = Colour(0x3b_a5_5d);
const COLOUR_PENDING: Colour = Colour(0xe6_b0_2e);
const COLOUR_ERROR: Colour = Colour(0xcf_3a_3a);
const COLOUR_NEUTRAL: Colour = Colour(0x5a_63_6e);

/// The title/colour/body of an embed, computed from a command outcome before
/// any serenity types are involved. Kept separate from [`CreateEmbed`] so the
/// outcome→presentation mapping is unit-testable without introspecting the
/// builder.
struct EmbedSpec {
    title: String,
    colour: Colour,
    body: String,
}

impl EmbedSpec {
    fn into_embed(self) -> CreateEmbed {
        CreateEmbed::new()
            .title(self.title)
            .colour(self.colour)
            .description(self.body)
    }
}

fn server_list_spec(servers: &[ServerSummary]) -> EmbedSpec {
    if servers.is_empty() {
        return EmbedSpec {
            title: "Game servers".to_owned(),
            colour: COLOUR_NEUTRAL,
            body: EMPTY_MESSAGE.to_owned(),
        };
    }

    let any_ready = servers.iter().any(|server| server.state == "Ready");
    let lines: Vec<String> = servers
        .iter()
        .map(|server| {
            let address = server.address.as_deref().unwrap_or(NO_ADDRESS);
            let game = server
                .game
                .as_deref()
                .map(|game| format!(" · {game}"))
                .unwrap_or_default();
            format!(
                "• **{}**{} — {} — `{}`",
                server.name, game, server.state, address
            )
        })
        .collect();
    EmbedSpec {
        title: "Game servers".to_owned(),
        colour: if any_ready { COLOUR_UP } else { COLOUR_NEUTRAL },
        body: lines.join("\n"),
    }
}

fn create_spec(outcome: &CreateOutcome, instance: &str) -> EmbedSpec {
    match outcome {
        CreateOutcome::Created { address, ready } => {
            started_spec(instance, address, *ready, "is up")
        }
        CreateOutcome::AlreadyExists => EmbedSpec {
            title: "Already running".to_owned(),
            colour: COLOUR_NEUTRAL,
            body: format!("A server named **{instance}** already exists."),
        },
        CreateOutcome::PortsExhausted => EmbedSpec {
            title: "No slots free".to_owned(),
            colour: COLOUR_ERROR,
            body: "All server slots are in use right now. Remove one first, then try again."
                .to_owned(),
        },
    }
}

fn start_spec(outcome: &StartOutcome, server: &str) -> EmbedSpec {
    match outcome {
        StartOutcome::Started { address, ready } => {
            started_spec(server, address, *ready, "is back up")
        }
        StartOutcome::AlreadyRunning => EmbedSpec {
            title: "Already running".to_owned(),
            colour: COLOUR_NEUTRAL,
            body: format!("**{server}** is already running."),
        },
        StartOutcome::NotFound => not_found_spec(server),
        StartOutcome::NotManaged => not_managed_spec(server),
        StartOutcome::UnknownGame(game) => EmbedSpec {
            title: "Game no longer available".to_owned(),
            colour: COLOUR_ERROR,
            body: format!("**{server}** runs '{game}', which is no longer in the catalog."),
        },
    }
}

fn stop_spec(outcome: &StopOutcome, server: &str) -> EmbedSpec {
    match outcome {
        StopOutcome::Stopped => EmbedSpec {
            title: format!("Stopped {server}"),
            colour: COLOUR_NEUTRAL,
            body: format!("Its world is saved. Use `/start` to bring **{server}** back later."),
        },
        StopOutcome::NotFound => not_found_spec(server),
        StopOutcome::NotManaged => not_managed_spec(server),
    }
}

fn remove_spec(outcome: &RemoveOutcome, server: &str) -> EmbedSpec {
    match outcome {
        RemoveOutcome::Removed => EmbedSpec {
            title: format!("Removed {server}"),
            colour: COLOUR_NEUTRAL,
            body: format!("**{server}** and its world have been deleted."),
        },
        RemoveOutcome::NotFound => not_found_spec(server),
        RemoveOutcome::NotManaged => not_managed_spec(server),
    }
}

/// Shared shape for "a server is now reachable" outcomes (create + start),
/// which differ only in the verb and whether the port is live yet.
fn started_spec(name: &str, address: &str, ready: bool, verb: &str) -> EmbedSpec {
    if ready {
        EmbedSpec {
            title: format!("🟢 {name} {verb}"),
            colour: COLOUR_UP,
            body: format!("Connect at `{address}`"),
        }
    } else {
        EmbedSpec {
            title: format!("🟡 {name} is starting"),
            colour: COLOUR_PENDING,
            body: format!("Connect at `{address}` in a couple of minutes."),
        }
    }
}

fn not_found_spec(server: &str) -> EmbedSpec {
    EmbedSpec {
        title: "No such server".to_owned(),
        colour: COLOUR_ERROR,
        body: format!("There's no server named **{server}**."),
    }
}

fn not_managed_spec(server: &str) -> EmbedSpec {
    EmbedSpec {
        title: "Off limits".to_owned(),
        colour: COLOUR_ERROR,
        body: format!("**{server}** is managed by the platform and can't be controlled from here."),
    }
}

pub(crate) fn server_list_embed(servers: &[ServerSummary]) -> CreateEmbed {
    server_list_spec(servers).into_embed()
}

pub(crate) fn create_result_embed(outcome: &CreateOutcome, instance: &str) -> CreateEmbed {
    create_spec(outcome, instance).into_embed()
}

pub(crate) fn start_result_embed(outcome: &StartOutcome, server: &str) -> CreateEmbed {
    start_spec(outcome, server).into_embed()
}

pub(crate) fn stop_result_embed(outcome: &StopOutcome, server: &str) -> CreateEmbed {
    stop_spec(outcome, server).into_embed()
}

pub(crate) fn remove_result_embed(outcome: &RemoveOutcome, server: &str) -> CreateEmbed {
    remove_spec(outcome, server).into_embed()
}

/// Red embed for "the operation broke" — the user-facing message must already
/// be plain-language and actionable; raw error detail belongs in the logs.
pub(crate) fn error_embed(message: &str) -> CreateEmbed {
    EmbedSpec {
        title: "Something went wrong".to_owned(),
        colour: COLOUR_ERROR,
        body: message.to_owned(),
    }
    .into_embed()
}

/// Amber "in progress" embed shown while a long operation runs, so the friend
/// sees the bot is working rather than staring at a silent multi-minute wait.
pub(crate) fn working_embed(title: &str, body: &str) -> CreateEmbed {
    EmbedSpec {
        title: format!("⏳ {title}"),
        colour: COLOUR_PENDING,
        body: body.to_owned(),
    }
    .into_embed()
}

/// Neutral embed for interstitial states (prompts, cancellations, timeouts).
pub(crate) fn neutral_embed(title: &str, body: &str) -> CreateEmbed {
    EmbedSpec {
        title: title.to_owned(),
        colour: COLOUR_NEUTRAL,
        body: body.to_owned(),
    }
    .into_embed()
}

/// Red warning shown before a destructive `/remove`, gating the deletion behind
/// an explicit Confirm/Cancel button press.
pub(crate) fn remove_confirm_embed(server: &str) -> CreateEmbed {
    EmbedSpec {
        title: format!("Delete {server}?"),
        colour: COLOUR_ERROR,
        body: format!("This permanently deletes **{server}** and its world. This can't be undone."),
    }
    .into_embed()
}

#[cfg(test)]
#[path = "tests/render.rs"]
mod tests;
