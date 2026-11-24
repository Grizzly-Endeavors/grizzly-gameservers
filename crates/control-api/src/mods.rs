use crate::package::{PackageId, PackageRelease, PackageVersion, ParseError};

pub const DEFAULT_WORK_DIR: &str = ".grizzly/thunderstore";

pub const RESOLVED_FILE: &str = "resolved";

pub const PROFILE_FILE: &str = "profile";

const CLIENT_ONLY_PREFIX: &str = "# client-only, not installed here: ";

const PROVIDED_PREFIX: &str = "# ";
const PROVIDED_SUFFIX: &str = " is supplied by the game image and was not installed";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolvedMods {
    pub installed: Vec<String>,
    pub client_only: Vec<String>,
    pub provided: Vec<String>,
}

impl ResolvedMods {
    #[must_use]
    pub fn render(&self) -> String {
        let mut text = String::new();
        for release in &self.installed {
            text.push_str(release);
            text.push('\n');
        }
        for release in &self.client_only {
            text.push_str(CLIENT_ONLY_PREFIX);
            text.push_str(release);
            text.push('\n');
        }
        for id in &self.provided {
            text.push_str(PROVIDED_PREFIX);
            text.push_str(id);
            text.push_str(PROVIDED_SUFFIX);
            text.push('\n');
        }
        text
    }

    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut parsed = Self::default();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(release) = line.strip_prefix(CLIENT_ONLY_PREFIX) {
                parsed.client_only.push(release.trim().to_owned());
            } else if let Some(id) = line
                .strip_prefix(PROVIDED_PREFIX)
                .and_then(|rest| rest.strip_suffix(PROVIDED_SUFFIX))
            {
                parsed.provided.push(id.trim().to_owned());
            } else if !line.starts_with('#') {
                parsed.installed.push(line.to_owned());
            }
        }
        parsed
    }
}

pub const CLIENT_PREFIX: &str = "client:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallLine {
    pub id: PackageId,
    pub pin: Option<PackageVersion>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientLineProblem {
    Unpinned(PackageId),
    Unusable(ParseError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModsLine {
    Ignored,
    Install(Result<InstallLine, ParseError>),
    Client(Result<PackageRelease, ClientLineProblem>),
}

#[must_use]
pub fn classify(raw: &str) -> ModsLine {
    let line = raw.trim();
    if line.is_empty() || line.starts_with('#') {
        return ModsLine::Ignored;
    }
    match line.strip_prefix(CLIENT_PREFIX) {
        Some(rest) => ModsLine::Client(classify_client(rest.trim())),
        None => ModsLine::Install(classify_install(line)),
    }
}

fn classify_install(line: &str) -> Result<InstallLine, ParseError> {
    if let Ok(release) = line.parse::<PackageRelease>() {
        return Ok(InstallLine {
            id: release.id,
            pin: Some(release.version),
        });
    }
    match line.parse::<PackageId>() {
        Ok(id) => Ok(InstallLine { id, pin: None }),
        Err(_) => Err(ParseError::NotARelease),
    }
}

fn classify_client(rest: &str) -> Result<PackageRelease, ClientLineProblem> {
    match rest.parse::<PackageRelease>() {
        Ok(release) => Ok(release),
        Err(reason) => Err(match rest.parse::<PackageId>() {
            Ok(id) => ClientLineProblem::Unpinned(id),
            Err(_) => ClientLineProblem::Unusable(reason),
        }),
    }
}

#[must_use]
pub fn render_install(id: &PackageId, pin: Option<PackageVersion>) -> String {
    match pin {
        Some(version) => format!("{id}-{version}"),
        None => id.to_string(),
    }
}

#[must_use]
pub fn render_client(release: &PackageRelease) -> String {
    format!("{CLIENT_PREFIX}{release}")
}

#[cfg(test)]
#[path = "tests/mods.rs"]
mod tests;
