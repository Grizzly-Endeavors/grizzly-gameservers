//! Turning an r2modman profile into a mod list, and a mod list back into a
//! profile. Pure — every registry fact arrives already looked up, so the whole
//! rule set is exercised without a network.
//!
//! The asymmetry between the two directions is the interesting part.
//!
//! **In**, a profile is a *client* profile: it was exported from somebody's
//! launcher, so it lists everything that player runs, including the things this
//! server has no business installing. Classifying each entry is what turns it
//! into a server's share of the pack.
//!
//! **Out**, the mod list is the group's whole modpack — this server's installs
//! *and* the entries it carries for the players' machines — so an export is the
//! union, minus the loader pack the image supplies. That omission is safe
//! because r2modman force-installs the community-pinned loader itself and the
//! mods that need it declare it as a dependency, so it arrives on the player's
//! machine either way; including it would mean inventing a version nobody here
//! knows, since the image's copy is whatever its updater last pulled.

use std::collections::BTreeMap;

use grizzly_control_api::mods::{CLIENT_PREFIX, ModsLine, ResolvedMods, classify};
use grizzly_control_api::package::PackageVersion;

use crate::registry::SideSupport;

use super::profile::ProfileMod;
use super::types::split_full_name;

/// What the registry had to say about one profile entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lookup {
    /// The registry has this package, and says where it has to be installed.
    Known(SideSupport),
    /// The registry has no package by that name.
    Missing,
}

/// A mod list built from a profile, and an account of what didn't make it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ImportPlan {
    /// Packages this server installs, as pinned `Namespace-Name-1.2.3` strings.
    pub(crate) install: Vec<String>,
    /// Packages the pack carries for the players' machines only.
    pub(crate) client_only: Vec<String>,
    /// Entries the exporting player had switched off.
    pub(crate) disabled: Vec<String>,
    /// Entries the game image already supplies.
    pub(crate) provided: Vec<String>,
    /// Entries this community has no record of.
    pub(crate) missing: Vec<String>,
}

impl ImportPlan {
    /// The file to write. Install lines first, then client lines, each sorted —
    /// so the same pack always produces the same file regardless of the order
    /// the exporting launcher happened to list it in.
    pub(crate) fn render(&self) -> String {
        let mut text = String::new();
        for line in &self.install {
            text.push_str(line);
            text.push('\n');
        }
        for line in &self.client_only {
            text.push_str(CLIENT_PREFIX);
            text.push_str(line);
            text.push('\n');
        }
        text
    }

    /// How many packages the list ends up naming, on either side.
    pub(crate) fn listed(&self) -> usize {
        self.install.len() + self.client_only.len()
    }
}

/// Classify a profile's entries into a mod list.
///
/// `provided` names the packages the game image supplies; `lookups` maps a
/// `Namespace-Name` to what the registry said about it, and an entry with no
/// lookup at all is treated as [`Lookup::Missing`] — the caller is expected to
/// have looked up everything it did not drop first.
///
/// The rule for an entry the registry didn't classify is to **install** it. That
/// asymmetry is deliberate: installing something the server didn't need costs a
/// download, while failing to install something it did need leaves the pack
/// broken in a way nobody can see — and "not stated" is the common case here,
/// since libraries are almost never tagged.
pub(crate) fn plan_import(
    mods: &[ProfileMod],
    provided: &[String],
    lookups: &BTreeMap<String, Lookup>,
) -> ImportPlan {
    let mut plan = ImportPlan::default();
    for entry in mods {
        let pinned = entry.pinned_name();
        if !entry.enabled {
            // A mod the exporting player chose not to run. Installing it would
            // be adopting a decision they had already made the other way.
            plan.disabled.push(pinned);
            continue;
        }
        if provided.contains(&entry.full_name) {
            plan.provided.push(pinned);
            continue;
        }
        match lookups.get(&entry.full_name) {
            Some(Lookup::Known(SideSupport::ClientOnly)) => plan.client_only.push(pinned),
            Some(Lookup::Known(_)) => plan.install.push(pinned),
            Some(Lookup::Missing) | None => plan.missing.push(pinned),
        }
    }
    plan.install.sort();
    plan.client_only.sort();
    plan
}

/// The packages that need looking up before [`plan_import`] can classify them —
/// everything the profile enables that the image doesn't already supply.
///
/// Split out so the caller makes exactly the registry calls the plan will use:
/// a disabled entry or the loader pack is dropped on facts already in hand, and
/// spending a round trip on it would be spending it on nothing.
pub(crate) fn lookups_needed(mods: &[ProfileMod], provided: &[String]) -> Vec<String> {
    let mut names: Vec<String> = mods
        .iter()
        .filter(|entry| entry.enabled)
        .map(|entry| entry.full_name.clone())
        .filter(|name| !provided.contains(name))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Build the profile an export mints a code for: everything the pack carries,
/// installed here or not, minus the packages the image supplies.
///
/// A `resolved` line that isn't a `Namespace-Name-1.2.3` release is skipped
/// rather than failing the export — the file is written by whatever supervisor
/// the pod is running, and one unreadable line should cost one entry.
pub(crate) fn plan_export(resolved: &ResolvedMods) -> Vec<ProfileMod> {
    let mut mods: Vec<ProfileMod> = resolved
        .installed
        .iter()
        .chain(&resolved.client_only)
        .filter_map(|release| split_release(release))
        .map(|(full_name, version)| ProfileMod {
            full_name,
            version,
            enabled: true,
        })
        .collect();
    // Sorted so the fingerprint below depends on the set and not on the order
    // the reconciler happened to write it in.
    mods.sort_by(|a, b| a.full_name.cmp(&b.full_name));
    mods
}

/// Which machines a mod-list entry belongs on. The distinction an import can
/// change without adding or removing anything, and the one a friend most needs
/// told: a mod that moved to the players' side is no longer running on the
/// server even though it is still in the pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Server,
    Players,
}

/// One package as a mod list declares it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Declared {
    side: Side,
    /// The pinned version, or `None` for an install line that takes the latest
    /// at every launch.
    version: Option<PackageVersion>,
}

/// What writing a new mod list changes about the one already on the server.
///
/// Against the *declared* list rather than against `resolved`, which is the
/// distinction that makes this readable: `resolved` also contains every
/// transitively-pulled dependency, none of which a new declared list names, so
/// diffing against it would report a dozen removals for a change that removed
/// nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ModsDiff {
    /// Packages the new list names and the old one didn't, as pinned releases.
    pub(crate) added: Vec<String>,
    /// Packages the old list named and the new one doesn't. The category an
    /// import can produce by accident, since it replaces rather than merges.
    pub(crate) removed: Vec<String>,
    /// Packages in both lists at a different version, as `Name old -> new`.
    pub(crate) changed: Vec<String>,
    /// Packages in both lists that swapped sides, as `Name: where it goes now`.
    pub(crate) moved: Vec<String>,
    /// Packages in both lists, same side, same version.
    pub(crate) unchanged: usize,
}

impl ModsDiff {
    /// Whether the write changes anything at all. A friend who asks for a pack
    /// the server is already running should be told that, not handed a list of
    /// nothing.
    pub(crate) fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.changed.is_empty()
            && self.moved.is_empty()
    }
}

/// Compare the mod list an import is about to write against the one on the
/// server now.
///
/// `current` is the raw text of the server's mod list — empty when it has none,
/// which is an ordinary state and reports as everything being added. Lines
/// `current` can't be parsed at all are skipped: the diff's job is to describe a
/// change, and a line that names no package can't have changed.
pub(crate) fn diff_import(current: &str, plan: &ImportPlan) -> ModsDiff {
    let before = declared(current);
    let after = planned(plan);

    let mut diff = ModsDiff::default();
    for (id, was) in &before {
        match after.get(id) {
            None => diff.removed.push(pinned(id, was.version)),
            Some(now) if now.side != was.side => {
                diff.moved.push(format!("{id}: {}", destination(now.side)));
            }
            Some(now) if now.version != was.version => diff.changed.push(format!(
                "{id} {} -> {}",
                version_text(was.version),
                version_text(now.version)
            )),
            Some(_) => diff.unchanged += 1,
        }
    }
    for (id, now) in &after {
        if !before.contains_key(id) {
            diff.added.push(pinned(id, now.version));
        }
    }
    diff
}

/// The packages a mod list's text declares. Built over the shared line grammar,
/// so what counts as an entry here is what the supervisor will install.
fn declared(text: &str) -> BTreeMap<String, Declared> {
    let mut declared = BTreeMap::new();
    for line in text.lines() {
        let (id, entry) = match classify(line) {
            ModsLine::Install(Ok(install)) => (
                install.id.to_string(),
                Declared {
                    side: Side::Server,
                    version: install.pin,
                },
            ),
            ModsLine::Client(Ok(release)) => (
                release.id.to_string(),
                Declared {
                    side: Side::Players,
                    version: Some(release.version),
                },
            ),
            ModsLine::Ignored | ModsLine::Install(Err(_)) | ModsLine::Client(Err(_)) => continue,
        };
        declared.insert(id, entry);
    }
    declared
}

/// The packages the import is about to write. Always pinned on both sides — the
/// plan is built from a profile, and a profile entry carries a version.
fn planned(plan: &ImportPlan) -> BTreeMap<String, Declared> {
    let sides = plan
        .install
        .iter()
        .map(|release| (release, Side::Server))
        .chain(
            plan.client_only
                .iter()
                .map(|release| (release, Side::Players)),
        );
    sides
        .filter_map(|(release, side)| {
            let (full_name, version) = split_release(release)?;
            Some((
                full_name,
                Declared {
                    side,
                    version: version.parse().ok(),
                },
            ))
        })
        .collect()
}

/// One package and its version, as a mod list would write it.
fn pinned(id: &str, version: Option<PackageVersion>) -> String {
    match version {
        Some(version) => format!("{id}-{version}"),
        None => id.to_owned(),
    }
}

/// A version for a diff line, naming the absence of one rather than eliding it:
/// an entry that was floating and is now pinned really did change.
fn version_text(version: Option<PackageVersion>) -> String {
    version.map_or_else(|| "unpinned".to_owned(), |version| version.to_string())
}

/// Where a package that changed sides goes now.
fn destination(side: Side) -> &'static str {
    match side {
        Side::Server => "now installed on the server",
        Side::Players => "now players' machines only",
    }
}

/// Split `Namespace-Name-1.2.3` into its pair and its version, rejecting
/// anything that isn't that shape.
fn split_release(release: &str) -> Option<(String, String)> {
    let (full_name, version) = release.rsplit_once('-')?;
    split_full_name(full_name)?;
    let numeric = version.split('.').count() == 3
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
    numeric.then(|| (full_name.to_owned(), version.to_owned()))
}

/// A minted code and the exact set it describes, as stored on the instance's
/// volume beside `resolved`.
///
/// The set is stored literally rather than hashed. It is a few hundred bytes,
/// comparing it is exact, and the file answers "what is in this code" without
/// anything having to re-derive it — which matters because there is no way to
/// revoke a code once it is out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CachedProfile {
    pub(crate) code: String,
    /// Pinned `Namespace-Name-1.2.3` releases, in the order the profile lists them.
    pub(crate) entries: Vec<String>,
}

/// Field naming the code in the cache file.
const CODE_FIELD: &str = "code:";

/// The one comment the cache file carries. Describes the format rather than a
/// moment, so it can't go stale.
const CACHE_HEADER: &str = "# The r2modman profile code last minted for this server, and the exact set it names.\n\
     # A code is permanent and cannot be revoked, so one is reused until the set changes.\n";

impl CachedProfile {
    pub(crate) fn render(&self) -> String {
        let mut text = String::from(CACHE_HEADER);
        text.push_str(CODE_FIELD);
        text.push(' ');
        text.push_str(&self.code);
        text.push('\n');
        for entry in &self.entries {
            text.push_str(entry);
            text.push('\n');
        }
        text
    }

    /// Read the cache file, or `None` if it isn't one.
    ///
    /// Lenient because this is a cache: an unreadable file must cost one extra
    /// mint, never an export.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let mut code = None;
        let mut entries = Vec::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            match line.strip_prefix(CODE_FIELD) {
                Some(value) if code.is_none() => code = Some(value.trim().to_owned()),
                Some(_) => return None,
                None => entries.push(line.to_owned()),
            }
        }
        let code = code.filter(|value| !value.is_empty())?;
        Some(Self { code, entries })
    }

    /// Whether this cached code still describes `mods`.
    pub(crate) fn describes(&self, mods: &[ProfileMod]) -> bool {
        self.entries.len() == mods.len()
            && self
                .entries
                .iter()
                .zip(mods)
                .all(|(entry, wanted)| *entry == wanted.pinned_name())
    }
}

/// The cache entries for a set about to be minted.
pub(crate) fn fingerprint(mods: &[ProfileMod]) -> Vec<String> {
    mods.iter().map(ProfileMod::pinned_name).collect()
}

#[cfg(test)]
#[path = "tests/modpack.rs"]
mod tests;
