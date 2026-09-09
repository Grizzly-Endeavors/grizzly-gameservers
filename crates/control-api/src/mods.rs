//! The `resolved` file the in-pod mod reconciler writes, and the one parser for
//! it. A file rather than a route, but a contract between the same two halves
//! for the same reason every route constant lives here: the supervisor renders
//! it after each launch and the bot reads it back through the file API, so the
//! two must agree on its shape or an export silently describes the wrong set.
//!
//! Three kinds of line, and the distinction between them is the whole point:
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

#[cfg(test)]
#[path = "tests/mods.rs"]
mod tests;
