//! The two mod files both halves read, and the one parser for each. Files
//! rather than routes, but contracts between the same two halves for the same
//! reason every route constant lives here — and each is read by the half that
//! did not write it, so a disagreement about shape is silent rather than an
//! error.
//!
//! **`.grizzly/mods`** is the declarative mod list. The supervisor parses it
//! before every launch to decide what to install; the bot parses it to rewrite a
//! pin ([`classify`]) and renders it from a profile code. One grammar with two
//! authors, so the grammar lives here and each half layers its own reporting on
//! top: the supervisor turns a bad install line into a launch failure naming the
//! line number, and the bot refuses to write one in the first place.
//!
//! **`resolved`** is what the last launch actually installed. The supervisor
//! renders it after each launch and the bot reads it back through the file API,
//! so the two must agree on its shape or an export silently describes the wrong
//! set.
//!
//! `resolved` has three kinds of line, and the distinction between them is the
//! whole point:
//!
//! - an **installed** release, one per line, which is what this instance runs;
//! - a **client-only** package, recorded but deliberately not installed here,
//!   because it belongs on the players' machines;
//! - a **provided** package, which the game image supplies and the reconciler
//!   skipped rather than installing a second copy over.
//!
//! Only the first two describe the group's modpack; the third exists so "why
//! isn't the loader pack in here" answers itself. Everything but the installed
//! releases is written as a `#` comment, so nothing that reads the file as a
//! plain list of what is installed can mistake one for the other.

use crate::package::{PackageId, PackageRelease, PackageVersion, ParseError};

/// Default directory the reconciler keeps its cache, staged tree, `resolved`
/// file, and minted profile code in, relative to the instance's data root.
///
/// Shared because the bot addresses these files by path through the file API and
/// has no other way to learn where they are: the supervisor takes the directory
/// from `SUPERVISOR_THUNDERSTORE_DIR`, which is a Dockerfile `ENV` baked into the
/// image and never visible on the pod. An image that overrides it hides
/// `resolved` from the bot, which is why the supervisor warns when one does.
pub const DEFAULT_WORK_DIR: &str = ".grizzly/thunderstore";

/// Name of the file recording what the last launch resolved, inside the working
/// directory.
pub const RESOLVED_FILE: &str = "resolved";

/// Name of the file caching the last profile code minted for this instance,
/// inside the working directory. Written by the bot, read by the bot; it lives
/// beside `resolved` because it is derived from it and rides the same
/// whole-volume archive.
pub const PROFILE_FILE: &str = "profile";

/// Comment prefix on a package recorded for players' machines only.
const CLIENT_ONLY_PREFIX: &str = "# client-only, not installed here: ";

/// Comment prefix and suffix around a package the game image supplies.
const PROVIDED_PREFIX: &str = "# ";
const PROVIDED_SUFFIX: &str = " is supplied by the game image and was not installed";

/// The whole modpack as the last launch resolved it.
///
/// Strings rather than parsed identifiers: this is the boundary between two
/// crates that model packages differently, and the grammar is already pinned by
/// whichever side is about to parse them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolvedMods {
    /// `Namespace-Name-1.2.3` releases installed on this server.
    pub installed: Vec<String>,
    /// `Namespace-Name-1.2.3` releases the modpack carries for players only.
    pub client_only: Vec<String>,
    /// `Namespace-Name` packages the image supplies. Versionless on purpose —
    /// the image owns whichever version it shipped, and nothing here knows it.
    pub provided: Vec<String>,
}

impl ResolvedMods {
    /// Render the file.
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

    /// Parse the file, ignoring anything it doesn't recognise.
    ///
    /// Lenient by design: this reads a file written by whatever supervisor
    /// version the pod happens to be running, and an unrecognised comment must
    /// degrade to "one less thing in the export" rather than to a failed read of
    /// a set that is otherwise entirely usable.
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

/// Marks a `.grizzly/mods` line as belonging to the players' machines rather
/// than to this server. Public because the bot renders lines with it and the
/// supervisor parses them with it.
pub const CLIENT_PREFIX: &str = "client:";

/// One package a `.grizzly/mods` install line names, with the version its
/// author pinned or `None` to take the latest at resolve time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallLine {
    pub id: PackageId,
    pub pin: Option<PackageVersion>,
}

/// Why a `client:` line couldn't be used.
///
/// Its own type rather than a bare [`ParseError`] because an unpinned entry is a
/// different mistake with a different fix — add a version, don't re-type the
/// line — and the generic "not a release" message would send someone looking
/// for a typo that isn't there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientLineProblem {
    /// Well-formed but unpinned, which a launcher can do nothing with.
    Unpinned(PackageId),
    /// Neither `Namespace-Name-1.2.3` nor anything close to it.
    Unusable(ParseError),
}

/// One line of `.grizzly/mods`, classified but not judged.
///
/// Deliberately stops at "what is this line", leaving what to *do* about a bad
/// one to each half: the same malformed text is a launch failure on an install
/// line and a skipped entry on a client line, and only the consumer knows which
/// of those it is in a position to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModsLine {
    /// Blank, or a `#` comment. Carried rather than dropped so a rewrite can put
    /// it back where it was.
    Ignored,
    /// A package this server installs, or the reason the line isn't one.
    Install(Result<InstallLine, ParseError>),
    /// A package the modpack carries for the players' machines only, or the
    /// reason the line isn't one.
    Client(Result<PackageRelease, ClientLineProblem>),
}

/// Classify one line of `.grizzly/mods`. Leading and trailing whitespace is
/// insignificant.
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

/// A package the server installs. Tries the pinned shape first: a bare
/// `Namespace-Name` never parses as a release (a release needs a numeric third
/// field), so the order can't misread one as the other.
fn classify_install(line: &str) -> Result<InstallLine, ParseError> {
    if let Ok(release) = line.parse::<PackageRelease>() {
        return Ok(InstallLine {
            id: release.id,
            pin: Some(release.version),
        });
    }
    match line.parse::<PackageId>() {
        Ok(id) => Ok(InstallLine { id, pin: None }),
        // Reported against the pinned shape, since that is the form the guidance
        // tells Gary to write and the form a typo is most likely aiming at.
        Err(_) => Err(ParseError::NotARelease),
    }
}

/// A package for the players' machines. Always an exact release, never a bare
/// id: nothing downstream resolves a client entry, so no later step could supply
/// the missing version — and a version is exactly what the launcher on the other
/// end needs.
fn classify_client(rest: &str) -> Result<PackageRelease, ClientLineProblem> {
    match rest.parse::<PackageRelease>() {
        Ok(release) => Ok(release),
        Err(reason) => Err(match rest.parse::<PackageId>() {
            Ok(id) => ClientLineProblem::Unpinned(id),
            Err(_) => ClientLineProblem::Unusable(reason),
        }),
    }
}

/// Render one install line, pinned when a version is given.
#[must_use]
pub fn render_install(id: &PackageId, pin: Option<PackageVersion>) -> String {
    match pin {
        Some(version) => format!("{id}-{version}"),
        None => id.to_string(),
    }
}

/// Render one `client:` line.
#[must_use]
pub fn render_client(release: &PackageRelease) -> String {
    format!("{CLIENT_PREFIX}{release}")
}

#[cfg(test)]
#[path = "tests/mods.rs"]
mod tests;
