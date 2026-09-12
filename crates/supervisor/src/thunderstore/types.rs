//! The one type this subsystem models that never leaves the pod: the
//! `manifest.json` inside a package zip.
//!
//! The identifiers themselves — [`PackageId`], [`PackageVersion`],
//! [`PackageRelease`] and their [`ParseError`] — live in
//! [`grizzly_control_api::package`] and are re-exported here, because the bot
//! authors the same strings when it rewrites a pin in `.grizzly/mods`. This
//! module keeps the re-export so every call site in the reconciler reads
//! `super::types::PackageRelease` whichever crate the grammar lives in.

use serde::Deserialize;

pub use grizzly_control_api::package::{PackageId, PackageRelease, PackageVersion, ParseError};

/// The `manifest.json` at the root of every Thunderstore package zip.
///
/// It carries the same `dependencies` strings the API reports, which is what
/// makes a cached zip enough to resolve a pinned entry with no network at all.
/// `name` is the package *name* alone — the namespace appears nowhere inside the
/// zip, so the cross-check against the filename can only pin the name half.
#[derive(Clone, Debug, Deserialize)]
pub struct ZipManifest {
    pub name: String,
    pub version_number: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
}

#[cfg(test)]
#[path = "tests/types.rs"]
mod tests;
