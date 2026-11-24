use std::collections::BTreeMap;

use grizzly_control_api::mods::{CLIENT_PREFIX, ModsLine, ResolvedMods, classify};
use grizzly_control_api::package::PackageVersion;

use crate::registry::SideSupport;

use super::profile::ProfileMod;
use super::types::split_full_name;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lookup {
    Known(SideSupport),
    Missing,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ImportPlan {
    pub(crate) install: Vec<String>,
    pub(crate) client_only: Vec<String>,
    pub(crate) disabled: Vec<String>,
    pub(crate) provided: Vec<String>,
    pub(crate) missing: Vec<String>,
}

impl ImportPlan {
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

    pub(crate) fn listed(&self) -> usize {
        self.install.len() + self.client_only.len()
    }
}

pub(crate) fn plan_import(
    mods: &[ProfileMod],
    provided: &[String],
    lookups: &BTreeMap<String, Lookup>,
) -> ImportPlan {
    let mut plan = ImportPlan::default();
    for entry in mods {
        let pinned = entry.pinned_name();
        if !entry.enabled {
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
    mods.sort_by(|a, b| a.full_name.cmp(&b.full_name));
    mods
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Server,
    Players,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Declared {
    side: Side,
    version: Option<PackageVersion>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ModsDiff {
    pub(crate) added: Vec<String>,
    pub(crate) removed: Vec<String>,
    pub(crate) changed: Vec<String>,
    pub(crate) moved: Vec<String>,
    pub(crate) unchanged: usize,
}

impl ModsDiff {
    pub(crate) fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.changed.is_empty()
            && self.moved.is_empty()
    }
}

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

fn pinned(id: &str, version: Option<PackageVersion>) -> String {
    match version {
        Some(version) => format!("{id}-{version}"),
        None => id.to_owned(),
    }
}

fn version_text(version: Option<PackageVersion>) -> String {
    version.map_or_else(|| "unpinned".to_owned(), |version| version.to_string())
}

fn destination(side: Side) -> &'static str {
    match side {
        Side::Server => "now installed on the server",
        Side::Players => "now players' machines only",
    }
}

fn split_release(release: &str) -> Option<(String, String)> {
    let (full_name, version) = release.rsplit_once('-')?;
    split_full_name(full_name)?;
    let numeric = version.split('.').count() == 3
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
    numeric.then(|| (full_name.to_owned(), version.to_owned()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CachedProfile {
    pub(crate) code: String,
    pub(crate) entries: Vec<String>,
}

const CODE_FIELD: &str = "code:";

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

    pub(crate) fn describes(&self, mods: &[ProfileMod]) -> bool {
        self.entries.len() == mods.len()
            && self
                .entries
                .iter()
                .zip(mods)
                .all(|(entry, wanted)| *entry == wanted.pinned_name())
    }
}

pub(crate) fn fingerprint(mods: &[ProfileMod]) -> Vec<String> {
    mods.iter().map(ProfileMod::pinned_name).collect()
}

#[cfg(test)]
#[path = "tests/modpack.rs"]
mod tests;
