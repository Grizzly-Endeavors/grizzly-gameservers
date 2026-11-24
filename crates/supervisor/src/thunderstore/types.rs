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
