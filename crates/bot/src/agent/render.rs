//! Shared text renderers for Gary's two surfaces (in-game chat and Discord).
//!
//! Both surfaces relay server summaries and the same error copy back through the
//! LLM. Keeping the rendering here — parameterized by [`GarySurface`] for the
//! *intentional* terseness differences — is what stops the two copies from
//! drifting. Unlike the rest of [`crate::agent`], this module references the
//! agones [`ServerSummary`] data type it renders; it stays pure (no cluster I/O)
//! and free of Discord.

use crate::agones::ServerSummary;

/// Which surface Gary is speaking on. In-game chat wants terser, single-line
/// copy than Discord, so the renderers branch on this instead of each surface
/// hand-rolling (and drifting) its own formatting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GarySurface {
    /// In-game chat: terse, positional, single-line-friendly.
    InGame,
    /// Discord: room for labeled, multi-line copy.
    Discord,
}

/// One server rendered as a single line. In-game uses a terse positional form;
/// Discord labels each field.
pub(crate) fn format_summary(surface: GarySurface, server: &ServerSummary) -> String {
    let game = server.game.as_deref().unwrap_or("unknown game");
    let address = server.address.as_deref().unwrap_or("no address yet");
    match surface {
        GarySurface::InGame => format!("{} ({game}, {}, {address})", server.name, server.state),
        GarySurface::Discord => format!(
            "{} (game: {game}, state: {}, address: {address})",
            server.name, server.state
        ),
    }
}

/// The active servers rendered as a list. In-game joins with "; " to stay on one
/// chat line; Discord joins with newlines.
pub(crate) fn format_server_list(surface: GarySurface, servers: &[ServerSummary]) -> String {
    if servers.is_empty() {
        return "no game servers are running right now".to_owned();
    }
    let separator = match surface {
        GarySurface::InGame => "; ",
        GarySurface::Discord => "\n",
    };
    servers
        .iter()
        .map(|server| format_summary(surface, server))
        .collect::<Vec<_>>()
        .join(separator)
}

/// Shared "unknown server" copy. It's a tool result the LLM reads on either
/// surface, so it carries the hint to re-list rather than differing per surface.
pub(crate) fn no_such(server: &str) -> String {
    format!("there's no server named {server} — check list_servers for the current names")
}

/// Shared cluster-unreachable copy for both surfaces.
pub(crate) fn cluster_error() -> String {
    "I couldn't reach the cluster just now — try again in a moment".to_owned()
}

#[cfg(test)]
#[path = "tests/render.rs"]
mod tests;
