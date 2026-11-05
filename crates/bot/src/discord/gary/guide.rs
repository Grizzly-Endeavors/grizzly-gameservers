//! Per-game configuration guidance, composed on demand.
//!
//! Two things about a game are genuinely per-game, and both are answered here.
//!
//! *Where its settings live.* Most games keep them in their own config files,
//! found by browsing; a game whose catalog entry declares a per-instance
//! overlay ([ADR-012](../../../docs/decisions/012-per-instance-env-overlay.md))
//! keeps some of them in the environment instead, where no amount of browsing
//! finds them. Answering that up front is what stops a search for a file that
//! does not exist.
//!
//! *How its mods work.* Mod management is per-game in the same way:
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

use crate::agones::{EnvOverlay, ModRegistry};
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
/// needs it. `overlay` is the game's per-instance settings surface as the
/// catalog declares it, so the key list Gary is shown is the one the supervisor
/// will actually honour rather than a copy of it kept in prompt text.
pub(crate) fn render_guide(
    game: &str,
    subject: &GuideSubject<'_>,
    overlay: Option<&EnvOverlay>,
    games: &str,
) -> String {
    // An id that isn't in the catalog has no settings and no mods to describe —
    // the only useful answer is the correction, so it returns before either
    // half runs.
    if matches!(subject, GuideSubject::UnknownGame) {
        return prompts::GameGuideUnknownGame { game, games }.render();
    }

    // Settings lead. Every real game answers here, and it is the half a question
    // about a password or a server name needs; the mod workflow follows for the
    // games that have one.
    let mut sections = vec![match overlay {
        Some(overlay) => prompts::GameGuideSettingsOverlay {
            game,
            file: &overlay.file,
            keys: &overlay.keys_sentence(),
        }
        .render(),
        None => prompts::GameGuideSettingsInFiles { game }.render(),
    }];

    let registry = match subject {
        // Returned above; repeated here because the match must be exhaustive and
        // a catch-all arm is denied.
        GuideSubject::UnknownGame => return sections.join(SECTION_GAP),
        GuideSubject::NoRegistry => {
            sections.push(prompts::GameGuideNoMods { game }.render());
            return sections.join(SECTION_GAP);
        }
        GuideSubject::Registry(registry) => registry,
    };
    sections.push(prompts::GameGuideIntro { game }.render());
    match registry {
        ModRegistry::Modrinth => {
            sections.push(prompts::GameGuideModrinthChoice::render());
            sections.push(prompts::GameGuideModrinthInstall::render());
        }
        ModRegistry::Thunderstore { .. } => {
            sections.push(prompts::GameGuideThunderstoreChoice::render());
            sections.push(prompts::GameGuideThunderstoreInstall::render());
            // Only this registry has profile codes, so this is the one block
            // with no Modrinth counterpart rather than half of a pair.
            sections.push(prompts::GameGuideThunderstoreProfiles::render());
        }
    }
    sections.join(SECTION_GAP)
}

#[cfg(test)]
#[path = "tests/guide.rs"]
mod tests;
