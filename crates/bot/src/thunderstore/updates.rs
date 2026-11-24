use std::collections::BTreeMap;

use grizzly_control_api::mods::{InstallLine, ModsLine, classify, render_client, render_install};
use grizzly_control_api::package::{PackageId, PackageRelease, PackageVersion};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Latest {
    Known {
        version: PackageVersion,
        deprecated: bool,
    },
    Missing,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct UpdatePlan {
    pub(crate) bumped: Vec<String>,
    pub(crate) client_bumped: Vec<String>,
    pub(crate) current: usize,
    pub(crate) floating: Vec<String>,
    pub(crate) deprecated: Vec<String>,
    pub(crate) unknown: Vec<String>,
    pub(crate) text: String,
}

impl UpdatePlan {
    pub(crate) fn is_empty(&self) -> bool {
        self.bumped.is_empty() && self.client_bumped.is_empty()
    }

    pub(crate) fn bumps(&self) -> usize {
        self.bumped.len() + self.client_bumped.len()
    }
}

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

pub(crate) fn plan_update(text: &str, latest: &BTreeMap<String, Latest>) -> UpdatePlan {
    let mut plan = UpdatePlan::default();
    let mut rewritten = String::with_capacity(text.len());
    for line in text.lines() {
        let replacement = match classify(line) {
            ModsLine::Install(Ok(install)) => bump_install(&mut plan, latest, &install),
            ModsLine::Client(Ok(release)) => bump_client(&mut plan, latest, &release),
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
