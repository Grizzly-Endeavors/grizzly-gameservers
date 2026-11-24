use crate::agones::{EnvOverlay, ModRegistry};
use crate::prompts;

pub(crate) enum GuideSubject<'a> {
    Registry(&'a ModRegistry),
    NoRegistry,
    UnknownGame,
}

const SECTION_GAP: &str = "\n\n";

pub(crate) fn render_guide(
    game: &str,
    subject: &GuideSubject<'_>,
    overlay: Option<&EnvOverlay>,
    games: &str,
) -> String {
    if matches!(subject, GuideSubject::UnknownGame) {
        return prompts::GameGuideUnknownGame { game, games }.render();
    }

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
            sections.push(prompts::GameGuideThunderstoreProfiles::render());
        }
    }
    sections.join(SECTION_GAP)
}

#[cfg(test)]
#[path = "tests/guide.rs"]
mod tests;
