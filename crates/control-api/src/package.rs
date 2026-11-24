use std::fmt;
use std::str::FromStr;

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
        let (id, version) = text.rsplit_once('-').ok_or(ParseError::NotARelease)?;
        Ok(Self {
            id: id.parse().map_err(|_part| ParseError::NotARelease)?,
            version: version.parse().map_err(|_part| ParseError::NotARelease)?,
        })
    }
}

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

#[cfg(test)]
#[path = "tests/package.rs"]
mod tests;
