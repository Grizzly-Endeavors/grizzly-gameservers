use poise::serenity_prelude as serenity;
use serenity::{Colour, CreateEmbed};

use crate::agones::{
    CreateOutcome, DestroyOutcome, ServerSummary, ShutdownOutcome, StartOutcome, SupervisorOutcome,
};
use crate::backup::{
    ArchiveOutcome, ArtifactSummary, BackupOutcome, RecoverOutcome, RestoreOutcome,
};

const EMPTY_MESSAGE: &str = "No game servers are running right now.";
const NO_ADDRESS: &str = "(not exposed yet)";

/// Discord rejects an embed whose description exceeds 4096 characters. A busy
/// guild's server list or a long backup list can blow past that.
const EMBED_DESCRIPTION_LIMIT: usize = 4096;

/// Join `lines` with newlines, capping the result at the embed-description
/// limit. If the full list would overflow, keep as many whole lines as fit and
/// append a "…and N more" tail, so an over-long list is clipped visibly instead
/// of failing the whole edit. Length is measured in bytes — always ≥ the
/// character count Discord actually caps on, so the bound stays conservative.
fn join_within_embed_limit(lines: &[String]) -> String {
    let full = lines.join("\n");
    if full.len() <= EMBED_DESCRIPTION_LIMIT {
        return full;
    }
    let mut out = String::new();
    for (shown, line) in lines.iter().enumerate() {
        let tail = format!("…and {} more", lines.len() - shown);
        let separator = usize::from(!out.is_empty());
        // Reserve room for this line and, after it, a newline + the tail.
        let projected = out.len() + separator + line.len() + 1 + tail.len();
        if projected > EMBED_DESCRIPTION_LIMIT {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&tail);
            return out;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
    }
    out
}

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

    let any_ready = servers.iter().any(|server| server.state.is_online());
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
        body: join_within_embed_limit(&lines),
    }
}

fn create_spec(outcome: &CreateOutcome, server: &str) -> EmbedSpec {
    match outcome {
        CreateOutcome::Created { address, ready } => started_spec(server, address, *ready, "is up"),
        CreateOutcome::AlreadyExists => EmbedSpec {
            title: "Already running".to_owned(),
            colour: COLOUR_NEUTRAL,
            body: format!("A server named **{server}** already exists."),
        },
        CreateOutcome::PortsExhausted => EmbedSpec {
            title: "No slots free".to_owned(),
            colour: COLOUR_ERROR,
            body: "All server slots are in use right now. Destroy one first, then try again."
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

fn shutdown_spec(outcome: &ShutdownOutcome, server: &str) -> EmbedSpec {
    match outcome {
        ShutdownOutcome::Down => EmbedSpec {
            title: format!("Stopped {server}"),
            colour: COLOUR_NEUTRAL,
            body: format!(
                "**{server}** is fully shut down and its world is saved. `/start` brings it back \
                 (a little slower than `/stop`'s pause, since it has to fully boot back up)."
            ),
        },
        ShutdownOutcome::NotFound => not_found_spec(server),
        ShutdownOutcome::NotManaged => not_managed_spec(server),
    }
}

/// Outcomes of the in-pod supervisor actions (`/stop`, `/start`, `/restart`).
/// Each carries the action in the variant, so one spec covers all three.
fn supervisor_spec(outcome: &SupervisorOutcome, server: &str) -> EmbedSpec {
    match outcome {
        SupervisorOutcome::Paused => EmbedSpec {
            title: format!("Paused {server}"),
            colour: COLOUR_NEUTRAL,
            body: format!(
                "**{server}** is paused — the world is saved and the server is kept warm, so \
                 `/start` brings it back in seconds."
            ),
        },
        SupervisorOutcome::Resumed => EmbedSpec {
            title: format!("🟡 {server} is waking up"),
            colour: COLOUR_PENDING,
            body: format!("**{server}** is loading its world back up — give it a few seconds."),
        },
        SupervisorOutcome::Restarted => EmbedSpec {
            title: format!("🔄 Restarted {server}"),
            colour: COLOUR_PENDING,
            body: format!("**{server}** is coming back up — give it a few seconds."),
        },
        SupervisorOutcome::AlreadyStopped => EmbedSpec {
            title: "Already paused".to_owned(),
            colour: COLOUR_NEUTRAL,
            body: format!("**{server}** is already paused. Use `/start` to bring it back."),
        },
        SupervisorOutcome::AlreadyRunning => EmbedSpec {
            title: "Already running".to_owned(),
            colour: COLOUR_NEUTRAL,
            body: format!("**{server}** is already running."),
        },
        SupervisorOutcome::PodNotReady => EmbedSpec {
            title: "Not ready yet".to_owned(),
            colour: COLOUR_ERROR,
            body: format!(
                "**{server}** isn't far enough along to control yet. Give it a moment and try again."
            ),
        },
        SupervisorOutcome::Unreachable => EmbedSpec {
            title: "Couldn't reach the server".to_owned(),
            colour: COLOUR_ERROR,
            body: format!(
                "I couldn't reach **{server}**'s controls right now. Try again in a moment."
            ),
        },
        SupervisorOutcome::Failed(message) => EmbedSpec {
            title: "Command failed".to_owned(),
            colour: COLOUR_ERROR,
            body: format!("**{server}**'s controls refused that: {message}"),
        },
        SupervisorOutcome::NotFound => not_found_spec(server),
        SupervisorOutcome::NotManaged => not_managed_spec(server),
    }
}

fn destroy_spec(outcome: &DestroyOutcome, server: &str) -> EmbedSpec {
    match outcome {
        DestroyOutcome::Destroyed => EmbedSpec {
            title: format!("Destroyed {server}"),
            colour: COLOUR_NEUTRAL,
            body: format!("**{server}** and its world have been deleted."),
        },
        DestroyOutcome::NotFound => not_found_spec(server),
        DestroyOutcome::NotManaged => not_managed_spec(server),
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

fn backup_spec(outcome: &BackupOutcome, server: &str) -> EmbedSpec {
    match outcome {
        BackupOutcome::BackedUp { size_bytes } => EmbedSpec {
            title: format!("💾 Backed up {server}"),
            colour: COLOUR_UP,
            body: format!(
                "Saved a {} backup. `/restore {server}` rolls it back to this (or an earlier) point.",
                human_size(*size_bytes)
            ),
        },
        BackupOutcome::NotRunning => EmbedSpec {
            title: "Nothing to back up".to_owned(),
            colour: COLOUR_ERROR,
            body: format!(
                "**{server}** isn't running, so there's nothing live to snapshot. `/start` it first."
            ),
        },
        BackupOutcome::Unreachable(_) => EmbedSpec {
            title: "Couldn't back it up".to_owned(),
            colour: COLOUR_ERROR,
            body: format!("I couldn't reach **{server}** to back it up. Try again in a moment."),
        },
        BackupOutcome::NotFound => not_found_spec(server),
        BackupOutcome::NotManaged => not_managed_spec(server),
    }
}

fn archive_spec(outcome: &ArchiveOutcome) -> EmbedSpec {
    match outcome {
        ArchiveOutcome::Archived { name, size_bytes } => EmbedSpec {
            title: format!("📦 Archived {name}"),
            colour: COLOUR_NEUTRAL,
            body: format!(
                "Saved a {} archive and released the server (its storage is freed). \
                 Use `/recover` to bring **{name}** back whenever you want.",
                human_size(*size_bytes)
            ),
        },
        ArchiveOutcome::Unavailable => backups_db_disabled_spec(),
        ArchiveOutcome::Failed(_) => EmbedSpec {
            title: "Couldn't archive it".to_owned(),
            colour: COLOUR_ERROR,
            body: "Something went wrong archiving that server, so nothing was released. \
                   Try again in a moment."
                .to_owned(),
        },
        ArchiveOutcome::NotFound => not_found_spec("that server"),
        ArchiveOutcome::NotManaged => not_managed_spec("that server"),
    }
}

fn restore_spec(outcome: &RestoreOutcome, server: &str) -> EmbedSpec {
    match outcome {
        RestoreOutcome::Restored { ready: true } => EmbedSpec {
            title: format!("🟢 Restored {server}"),
            colour: COLOUR_UP,
            body: format!("**{server}** is back up on the restored world."),
        },
        RestoreOutcome::Restored { ready: false } => EmbedSpec {
            title: format!("🟡 {server} is coming back"),
            colour: COLOUR_PENDING,
            body: format!("Restored the world onto **{server}** — it'll be playable in a minute."),
        },
        RestoreOutcome::Failed(_) => EmbedSpec {
            title: "Restore failed".to_owned(),
            colour: COLOUR_ERROR,
            body: format!("I couldn't restore **{server}** cleanly. Try again in a moment."),
        },
        RestoreOutcome::NotFound => not_found_spec(server),
        RestoreOutcome::NotManaged => not_managed_spec(server),
    }
}

fn recover_spec(outcome: &RecoverOutcome, name: &str) -> EmbedSpec {
    match outcome {
        RecoverOutcome::Recovered { address, ready } => {
            started_spec(name, address, *ready, "is back")
        }
        RecoverOutcome::NoSuchArchive => EmbedSpec {
            title: "No such archive".to_owned(),
            colour: COLOUR_ERROR,
            body: format!("There's no archived server named **{name}** in this Discord server."),
        },
        RecoverOutcome::NameInUse => EmbedSpec {
            title: "Name in use".to_owned(),
            colour: COLOUR_ERROR,
            body: format!(
                "A server named **{name}** is already running. Pick it up with `/start`."
            ),
        },
        RecoverOutcome::Unavailable => backups_db_disabled_spec(),
        RecoverOutcome::UnknownGame(game) => EmbedSpec {
            title: "Game no longer available".to_owned(),
            colour: COLOUR_ERROR,
            body: format!("**{name}** ran '{game}', which is no longer in the catalog."),
        },
        RecoverOutcome::PortsExhausted => EmbedSpec {
            title: "No slots free".to_owned(),
            colour: COLOUR_ERROR,
            body: "All server slots are in use right now. Destroy or archive one first.".to_owned(),
        },
        RecoverOutcome::Failed(_) => EmbedSpec {
            title: "Recover failed".to_owned(),
            colour: COLOUR_ERROR,
            body: format!("I couldn't bring **{name}** back cleanly. Try again in a moment."),
        },
    }
}

/// A list of a server's backups or a server's archives, newest first.
fn artifact_list_spec(title: &str, artifacts: &[ArtifactSummary], empty: &str) -> EmbedSpec {
    if artifacts.is_empty() {
        return EmbedSpec {
            title: title.to_owned(),
            colour: COLOUR_NEUTRAL,
            body: empty.to_owned(),
        };
    }
    let lines: Vec<String> = artifacts
        .iter()
        .map(|artifact| {
            format!(
                "• **{}** — {} — {}",
                artifact.name,
                human_size(artifact.size_bytes),
                artifact.created_at
            )
        })
        .collect();
    EmbedSpec {
        title: title.to_owned(),
        colour: COLOUR_NEUTRAL,
        body: join_within_embed_limit(&lines),
    }
}

/// Shown when a backup command is invoked but S3 isn't configured at all.
fn backups_disabled_spec() -> EmbedSpec {
    EmbedSpec {
        title: "Backups aren't set up".to_owned(),
        colour: COLOUR_ERROR,
        body: "Backups aren't configured on this bot yet, so there's nothing to save or restore."
            .to_owned(),
    }
}

/// Shown when archive/recover is invoked but the archive catalog (database) is off.
fn backups_db_disabled_spec() -> EmbedSpec {
    EmbedSpec {
        title: "Archives aren't available".to_owned(),
        colour: COLOUR_ERROR,
        body: "I can't track archives right now — my archive records are offline. \
               Backups and restore still work; try archiving again later."
            .to_owned(),
    }
}

/// Format a byte count as a friendly size (`B`/`KiB`/`MiB`/`GiB`) using integer
/// math — a fractional GiB is display-only, so no float cast is needed.
pub(crate) fn human_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    if bytes >= GIB {
        scaled_size(bytes, GIB, "GiB")
    } else if bytes >= MIB {
        scaled_size(bytes, MIB, "MiB")
    } else if bytes >= KIB {
        scaled_size(bytes, KIB, "KiB")
    } else {
        format!("{bytes} B")
    }
}

/// One-decimal size in the given unit, computed without floats: the tenths come
/// from the remainder scaled by ten.
fn scaled_size(bytes: u64, divisor: u64, unit: &str) -> String {
    let whole = bytes / divisor;
    let tenths = (bytes % divisor) * 10 / divisor;
    format!("{whole}.{tenths} {unit}")
}

fn not_found_spec(server: &str) -> EmbedSpec {
    EmbedSpec {
        title: "No such server".to_owned(),
        colour: COLOUR_ERROR,
        body: format!(
            "There's no server named **{server}**. Check `/servers` for the current names."
        ),
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

pub(crate) fn create_result_embed(outcome: &CreateOutcome, server: &str) -> CreateEmbed {
    create_spec(outcome, server).into_embed()
}

pub(crate) fn start_result_embed(outcome: &StartOutcome, server: &str) -> CreateEmbed {
    start_spec(outcome, server).into_embed()
}

pub(crate) fn shutdown_result_embed(outcome: &ShutdownOutcome, server: &str) -> CreateEmbed {
    shutdown_spec(outcome, server).into_embed()
}

pub(crate) fn supervisor_result_embed(outcome: &SupervisorOutcome, server: &str) -> CreateEmbed {
    supervisor_spec(outcome, server).into_embed()
}

pub(crate) fn destroy_result_embed(outcome: &DestroyOutcome, server: &str) -> CreateEmbed {
    destroy_spec(outcome, server).into_embed()
}

pub(crate) fn backup_result_embed(outcome: &BackupOutcome, server: &str) -> CreateEmbed {
    backup_spec(outcome, server).into_embed()
}

pub(crate) fn archive_result_embed(outcome: &ArchiveOutcome) -> CreateEmbed {
    archive_spec(outcome).into_embed()
}

pub(crate) fn restore_result_embed(outcome: &RestoreOutcome, server: &str) -> CreateEmbed {
    restore_spec(outcome, server).into_embed()
}

pub(crate) fn recover_result_embed(outcome: &RecoverOutcome, name: &str) -> CreateEmbed {
    recover_spec(outcome, name).into_embed()
}

pub(crate) fn backups_list_embed(server: &str, artifacts: &[ArtifactSummary]) -> CreateEmbed {
    artifact_list_spec(
        &format!("Backups of {server}"),
        artifacts,
        &format!("**{server}** has no backups yet. Use `/backup {server}` to take one."),
    )
    .into_embed()
}

pub(crate) fn archives_list_embed(artifacts: &[ArtifactSummary]) -> CreateEmbed {
    artifact_list_spec(
        "Archived servers",
        artifacts,
        "No servers are archived in this Discord server. `/archive` puts one into cold storage.",
    )
    .into_embed()
}

pub(crate) fn backups_disabled_embed() -> CreateEmbed {
    backups_disabled_spec().into_embed()
}

pub(crate) fn archives_unavailable_embed() -> CreateEmbed {
    backups_db_disabled_spec().into_embed()
}

/// Red warning shown before an overwrite-restore, gating it behind a confirm.
pub(crate) fn restore_confirm_embed(server: &str, label: &str) -> CreateEmbed {
    EmbedSpec {
        title: format!("Restore {server}?"),
        colour: COLOUR_ERROR,
        body: format!(
            "This replaces **{server}**'s current world with the backup from **{label}**. \
             I'll take a safety backup of the current world first, but the live world will be \
             overwritten. Continue?"
        ),
    }
    .into_embed()
}

/// Red warning shown before archiving, which releases the server's storage.
pub(crate) fn archive_confirm_embed(server: &str) -> CreateEmbed {
    EmbedSpec {
        title: format!("Archive {server}?"),
        colour: COLOUR_ERROR,
        body: format!(
            "This stops **{server}**, saves a durable backup, and releases its storage. \
             The world is kept safe in the archive — `/recover` brings it back — but the running \
             server goes away. Continue?"
        ),
    }
    .into_embed()
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

/// The shared refusal shown when a guild-scoped command runs without a guild (a
/// DM). One home so the "run this in a server" copy doesn't drift across
/// `/servers`, `/archives`, `/recover`, `/config`, and the scope check.
pub(crate) fn guild_required_embed() -> CreateEmbed {
    neutral_embed(
        "Run this in a server",
        "These commands only work inside a Discord server — run this in one of its channels, not a direct message.",
    )
}

/// Red warning shown before a destructive `/destroy`, gating the deletion behind
/// an explicit Confirm/Cancel button press.
pub(crate) fn destroy_confirm_embed(server: &str) -> CreateEmbed {
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
