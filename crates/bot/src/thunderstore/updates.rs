//! Bumping every pinned version in a mod list at once. Pure — the registry's
//! answer for each package arrives already looked up, so every rewrite rule is
//! exercised without a network.
//!
//! **The rewrite is line-by-line, not a re-render.** The list is a file a friend
//! or the agent authored: it carries comments, an order somebody chose, and
//! sometimes a line nothing here understands. Rebuilding it from a parsed set
//! would silently discard all of that, so each line is replaced in place or left
//! exactly as it was, and a line that isn't a package is never touched.
//!
//! **A pin only ever goes up.** The registry reports the latest version, and
//! "latest" is the only claim it makes — a list pinned above it (a version
//! yanked after someone pinned it, or a package whose author re-published
//! downward) is left alone rather than quietly rolled back to something older
//! than what the server is running.
//!
//! **An unpinned line has nothing to bump.** It already resolves to the latest
//! at every launch, which is a different mechanism reaching the same place; it
//! is reported as such rather than pinned, because pinning it would *remove*
//! that behaviour under the name of an update.

use std::collections::BTreeMap;

use grizzly_control_api::mods::{InstallLine, ModsLine, classify, render_client, render_install};
use grizzly_control_api::package::{PackageId, PackageRelease, PackageVersion};

/// What the registry says about one package right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Latest {
    /// The newest version, and whether the package has been deprecated.
    Known {
        version: PackageVersion,
        deprecated: bool,
    },
    /// The registry has no package by that name. Its line is left alone: the
    /// list may be older than a rename, and rewriting a line whose package
    /// can't be confirmed is how an update breaks a launch.
    Missing,
}

/// What updating a mod list would change, and the file it would write.
///
/// Every category is a different thing to tell a friend, and the split between
/// the two bumped lists is the consequential one: a server-side bump lands on
/// the next restart and needs nothing from anyone, while a players'-side bump is
/// work every player has to do in their own mod manager before they can connect.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct UpdatePlan {
    /// Server-side pins raised, as `Namespace-Name old -> new`.
    pub(crate) bumped: Vec<String>,
    /// Players'-machines pins raised, same shape.
    pub(crate) client_bumped: Vec<String>,
    /// Packages already at the newest version.
    pub(crate) current: usize,
    /// Install lines with no pin, which already take the latest at every launch.
    pub(crate) floating: Vec<String>,
    /// Packages whose latest version is now deprecated by its author. Reported
    /// whether or not the pin moved, because it is a reason to stop carrying a
    /// mod and nothing else in the conversation will mention it.
    pub(crate) deprecated: Vec<String>,
    /// Packages the registry has no record of. Their lines are left untouched.
    pub(crate) unknown: Vec<String>,
    /// The file to write. Identical to the input when nothing moved.
    pub(crate) text: String,
}

impl UpdatePlan {
    /// Whether anything would actually be rewritten.
    pub(crate) fn is_empty(&self) -> bool {
        self.bumped.is_empty() && self.client_bumped.is_empty()
    }

    /// How many pins would move, on either side.
    pub(crate) fn bumps(&self) -> usize {
        self.bumped.len() + self.client_bumped.len()
    }
}

/// Every package in the list whose version an update could move — the pinned
/// ones, on either side.
///
/// Split out so the caller makes exactly the registry calls the plan will use:
/// an unpinned line and a comment are decided on facts already in hand, and
/// spending a round trip on either would be spending it on nothing.
pub(crate) fn lookups_needed(text: &str) -> Vec<PackageId> {
    let mut ids: Vec<PackageId> = text
        .lines()
        .filter_map(|line| match classify(line) {
            ModsLine::Install(Ok(install)) => install.pin.map(|_| install.id),
            ModsLine::Client(Ok(release)) => Some(release.id),
            ModsLine::Ignored | ModsLine::Install(Err(_)) | ModsLine::Client(Err(_)) => None,
        })
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// Rewrite a mod list onto the newest version of everything it pins.
///
/// `latest` maps a `Namespace-Name` to what the registry reported. A package
/// with no entry at all is treated as [`Latest::Missing`] — the caller is
/// expected to have looked up everything [`lookups_needed`] named.
pub(crate) fn plan_update(text: &str, latest: &BTreeMap<String, Latest>) -> UpdatePlan {
    let mut plan = UpdatePlan::default();
    let mut rewritten = String::with_capacity(text.len());
    for line in text.lines() {
        let replacement = match classify(line) {
            ModsLine::Install(Ok(install)) => bump_install(&mut plan, latest, &install),
            ModsLine::Client(Ok(release)) => bump_client(&mut plan, latest, &release),
            // A comment, a blank line, or something nothing here understands.
            // Carried through byte for byte — an update has no business editing
            // a line it can't read.
            ModsLine::Ignored | ModsLine::Install(Err(_)) | ModsLine::Client(Err(_)) => None,
        };
        match replacement {
            Some(new_line) => rewritten.push_str(&new_line),
            None => rewritten.push_str(line),
        }
        rewritten.push('\n');
    }
    plan.text = rewritten;
    plan
}

/// Raise one install line's pin, or account for why it stays.
fn bump_install(
    plan: &mut UpdatePlan,
    latest: &BTreeMap<String, Latest>,
    install: &InstallLine,
) -> Option<String> {
    let id = &install.id;
    let Some(pin) = install.pin else {
        plan.floating.push(id.to_string());
        return None;
    };
    let newest = note_lookup(plan, latest, id)?;
    if newest <= pin {
        plan.current += 1;
        return None;
    }
    plan.bumped.push(format!("{id} {pin} -> {newest}"));
    Some(render_install(id, Some(newest)))
}

/// Raise one `client:` line's pin, or account for why it stays.
fn bump_client(
    plan: &mut UpdatePlan,
    latest: &BTreeMap<String, Latest>,
    release: &PackageRelease,
) -> Option<String> {
    let newest = note_lookup(plan, latest, &release.id)?;
    if newest <= release.version {
        plan.current += 1;
        return None;
    }
    plan.client_bumped
        .push(format!("{} {} -> {newest}", release.id, release.version));
    Some(render_client(&PackageRelease {
        id: release.id.clone(),
        version: newest,
    }))
}

/// The newest version of `id`, recording the two things a lookup can report
/// alongside it. `None` means the line must be left as it is.
fn note_lookup(
    plan: &mut UpdatePlan,
    latest: &BTreeMap<String, Latest>,
    id: &PackageId,
) -> Option<PackageVersion> {
    match latest.get(&id.to_string()) {
        Some(Latest::Known {
            version,
            deprecated,
        }) => {
            if *deprecated {
                plan.deprecated.push(id.to_string());
            }
            Some(*version)
        }
        Some(Latest::Missing) | None => {
            plan.unknown.push(id.to_string());
            None
        }
    }
}

#[cfg(test)]
#[path = "tests/updates.rs"]
mod tests;
