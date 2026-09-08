//! The identifiers this subsystem is built on: a package's `Namespace-Name`
//! pair, its three-part version, the `Namespace-Name-Major.Minor.Patch` strings
//! Thunderstore uses for dependencies, and the `manifest.json` every package zip
//! carries. Pure — parsing and formatting only.
//!
//! The grammar is deliberately narrower than semver: Thunderstore versions are
//! exactly three numeric parts, and accepting anything else (a pre-release
//! suffix, a two-part version) would let a malformed mod-list entry through to
//! become a 404 at launch instead of a named parse error at the line that wrote
//! it.

use std::fmt;
use std::str::FromStr;

use serde::Deserialize;

/// A package's identity within a community: `Namespace-Name`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageId {
    pub namespace: String,
    pub name: String,
}

impl fmt::Display for PackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.namespace, self.name)
    }
}

/// Whether a component of an identifier is one Thunderstore would accept:
/// non-empty ASCII alphanumerics and underscores. The dash is excluded on
/// purpose — it is the separator, and allowing it in a component would make
/// every identifier ambiguous.
fn is_component(part: &str) -> bool {
    !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl FromStr for PackageId {
    type Err = ParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (namespace, name) = text.split_once('-').ok_or(ParseError::NotAPackageId)?;
        if !is_component(namespace) || !is_component(name) {
            return Err(ParseError::NotAPackageId);
        }
        Ok(Self {
            namespace: namespace.to_owned(),
            name: name.to_owned(),
        })
    }
}

/// A package version: exactly three numeric parts, ordered numerically.
///
/// The derived `Ord` compares major, then minor, then patch — which is why the
/// fields are declared in that order and must stay that way. Numeric ordering is
/// what makes "the highest version wins" mean what it says; a string comparison
/// would put `1.10.0` below `1.9.0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl fmt::Display for PackageVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl FromStr for PackageVersion {
    type Err = ParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut parts = text.split('.');
        let mut next = || -> Result<u64, ParseError> {
            parts
                .next()
                .ok_or(ParseError::NotAVersion)?
                .parse::<u64>()
                .map_err(|_digits| ParseError::NotAVersion)
        };
        let version = Self {
            major: next()?,
            minor: next()?,
            patch: next()?,
        };
        if parts.next().is_some() {
            return Err(ParseError::NotAVersion);
        }
        Ok(version)
    }
}

/// One exact package release: `Namespace-Name-Major.Minor.Patch`. The shape of
/// every dependency string, every cache filename, and every line of `resolved`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageRelease {
    pub id: PackageId,
    pub version: PackageVersion,
}

impl fmt::Display for PackageRelease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.id, self.version)
    }
}

impl FromStr for PackageRelease {
    type Err = ParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        // Split from the right: the version is the last dash-separated field,
        // and the namespace and name each contain no dash of their own.
        let (id, version) = text.rsplit_once('-').ok_or(ParseError::NotARelease)?;
        Ok(Self {
            id: id.parse().map_err(|_part| ParseError::NotARelease)?,
            version: version.parse().map_err(|_part| ParseError::NotARelease)?,
        })
    }
}

/// Why an identifier didn't parse. Each variant names the shape that was
/// expected, because these strings reach a friend through a launch-failure log
/// line and "invalid" alone would not tell them what to type instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    NotAPackageId,
    NotAVersion,
    NotARelease,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAPackageId => f.write_str(
                "expected a package as Namespace-Name (letters, digits and underscores)",
            ),
            Self::NotAVersion => f.write_str("expected a version as three numbers, e.g. 1.2.3"),
            Self::NotARelease => f.write_str(
                "expected a package as Namespace-Name-1.2.3 (letters, digits and underscores)",
            ),
        }
    }
}

impl std::error::Error for ParseError {}

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
