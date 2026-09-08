//! Per-game mod guidance, composed on demand.
//!
//! Mod management is the one part of Gary's job that is genuinely per-game:
//! different identifiers, a different file to write, and — between Modrinth and
//! Thunderstore — opposite rules for removing something. That knowledge used to
//! ride the system prompt on every manager turn in every guild, whether or not
//! the guild ran the game it described, and adding the second game would have
//! doubled it. Loading it through a tool moves the cost to the turns that
//! actually need it and makes the third game free (closes issue #61).
//!
//! Pure: the composition is a `&str` in and a `String` out, so every branch is
//! covered without a catalog or a cluster. Sections are joined here rather than
//! inside the prompt files, so a later per-game quirk slots in as another block
//! instead of a fork of an existing one.

use crate::agones::ModRegistry;
use crate::prompts;

/// What the catalog says about the game a guide was asked for.
pub(crate) enum GuideSubject<'a> {
    /// A real game with a registry behind it.
    Registry(&'a ModRegistry),
    /// A real game that declares no registry — a final answer, not a gap.
    NoRegistry,
    /// Nothing in the catalog by that name — a typo to correct.
    UnknownGame,
}

/// Separator between guide sections. Owned here because prompt files carry no
/// leading or trailing whitespace, the same convention the system prompt uses.
const SECTION_GAP: &str = "\n\n";

/// Render the guide for one game.
///
/// `games` is the comma-joined catalog listing, used only by the unknown-game
/// branch; the caller computes it once rather than this deciding whether it
/// needs it.
pub(crate) fn render_guide(game: &str, subject: &GuideSubject<'_>, games: &str) -> String {
    let registry = match subject {
        GuideSubject::UnknownGame => {
            return prompts::GameGuideUnknownGame { game, games }.render();
        }
        GuideSubject::NoRegistry => return prompts::GameGuideNoMods { game }.render(),
        GuideSubject::Registry(registry) => registry,
    };
    let (choice, install) = match registry {
        ModRegistry::Modrinth => (
            prompts::GameGuideModrinthChoice::render(),
            prompts::GameGuideModrinthInstall::render(),
        ),
        ModRegistry::Thunderstore { .. } => (
            prompts::GameGuideThunderstoreChoice::render(),
            prompts::GameGuideThunderstoreInstall::render(),
        ),
    };
    [prompts::GameGuideIntro { game }.render(), choice, install].join(SECTION_GAP)
}

#[cfg(test)]
#[path = "tests/guide.rs"]
mod tests;
