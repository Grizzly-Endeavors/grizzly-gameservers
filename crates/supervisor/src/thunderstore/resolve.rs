use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::future::Future;

use super::types::{PackageId, PackageRelease, PackageVersion};

pub const MAX_PACKAGES: usize = 256;

pub const MAX_DEPTH: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceVersion {
    pub version: PackageVersion,
    pub dependencies: Vec<PackageRelease>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceError {
    NotFound,
    Unavailable(String),
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("the registry has no such package or version"),
            Self::Unavailable(detail) => write!(f, "the registry could not be reached ({detail})"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Requester {
    Line(usize),
    Dependency(PackageRelease),
}

impl fmt::Display for Requester {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Line(number) => write!(f, "line {number} of the mod list"),
            Self::Dependency(release) => write!(f, "{release}, which needs it"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveWarning {
    PinHeldBelowDependency {
        pinned: PackageRelease,
        wanted: PackageVersion,
        requested_by: Requester,
    },
}

impl fmt::Display for ResolveWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PinHeldBelowDependency {
                pinned,
                wanted,
                requested_by,
            } => write!(
                f,
                "{pinned} is pinned, but {requested_by} was built against {wanted}; \
                 keeping the pin"
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownPackage {
    pub id: PackageId,
    pub version: Option<PackageVersion>,
    pub requested_by: Requester,
    pub cause: SourceError,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    Unknown(Box<UnknownPackage>),
    TooManyPackages { limit: usize },
    TooDeep { limit: usize, id: PackageId },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(unknown) => {
                let UnknownPackage {
                    id,
                    version,
                    requested_by,
                    cause,
                } = unknown.as_ref();
                let what = version.map_or_else(|| id.to_string(), |v| format!("{id}-{v}"));
                write!(f, "{what} (from {requested_by}): {cause}")
            }
            Self::TooManyPackages { limit } => write!(
                f,
                "this mod list pulls in more than {limit} packages; \
                 something in it is almost certainly wrong"
            ),
            Self::TooDeep { limit, id } => write!(
                f,
                "dependencies nest more than {limit} deep at {id}; \
                 something in them is almost certainly wrong"
            ),
        }
    }
}

impl std::error::Error for ResolveError {}

pub trait PackageSource {
    fn latest(
        &self,
        id: &PackageId,
    ) -> impl Future<Output = Result<SourceVersion, SourceError>> + Send;

    fn exact(
        &self,
        release: &PackageRelease,
    ) -> impl Future<Output = Result<SourceVersion, SourceError>> + Send;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPackage {
    pub release: PackageRelease,
    pub pinned: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resolution {
    pub packages: Vec<ResolvedPackage>,
    pub provided: Vec<PackageId>,
    pub warnings: Vec<ResolveWarning>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wanted {
    pub id: PackageId,
    pub pin: Option<PackageVersion>,
    pub line_number: usize,
}

struct Want {
    id: PackageId,
    version: PackageVersion,
    depth: usize,
    requested_by: Requester,
}

struct Decided {
    version: PackageVersion,
    pinned: bool,
}

pub async fn resolve<S: PackageSource>(
    source: &S,
    wanted: &[Wanted],
    provided: &[PackageId],
) -> Result<Resolution, ResolveError> {
    let mut decided: BTreeMap<PackageId, Decided> = BTreeMap::new();
    let mut skipped: BTreeSet<PackageId> = BTreeSet::new();
    let mut warnings = Vec::new();
    let mut queue: Vec<Want> = Vec::new();

    for entry in wanted {
        if provided.contains(&entry.id) {
            skipped.insert(entry.id.clone());
            continue;
        }
        let requested_by = Requester::Line(entry.line_number);
        let found = fetch(source, &entry.id, entry.pin, &requested_by).await?;
        decided.insert(
            entry.id.clone(),
            Decided {
                version: found.version,
                pinned: entry.pin.is_some(),
            },
        );
        enqueue_dependencies(&mut queue, &entry.id, &found, 0);
    }
    if decided.len() > MAX_PACKAGES {
        return Err(ResolveError::TooManyPackages {
            limit: MAX_PACKAGES,
        });
    }

    while let Some(want) = queue.pop() {
        if provided.contains(&want.id) {
            skipped.insert(want.id);
            continue;
        }
        if want.depth > MAX_DEPTH {
            return Err(ResolveError::TooDeep {
                limit: MAX_DEPTH,
                id: want.id,
            });
        }

        if let Some(current) = decided.get(&want.id) {
            if want.version <= current.version {
                continue;
            }
            if current.pinned {
                warnings.push(ResolveWarning::PinHeldBelowDependency {
                    pinned: PackageRelease {
                        id: want.id,
                        version: current.version,
                    },
                    wanted: want.version,
                    requested_by: want.requested_by,
                });
                continue;
            }
        }

        let found = fetch(source, &want.id, Some(want.version), &want.requested_by).await?;
        let first_time = decided
            .insert(
                want.id.clone(),
                Decided {
                    version: found.version,
                    pinned: false,
                },
            )
            .is_none();
        if first_time && decided.len() > MAX_PACKAGES {
            return Err(ResolveError::TooManyPackages {
                limit: MAX_PACKAGES,
            });
        }
        enqueue_dependencies(&mut queue, &want.id, &found, want.depth);
    }

    Ok(Resolution {
        packages: decided
            .into_iter()
            .map(|(id, decided)| ResolvedPackage {
                release: PackageRelease {
                    id,
                    version: decided.version,
                },
                pinned: decided.pinned,
            })
            .collect(),
        provided: skipped.into_iter().collect(),
        warnings,
    })
}

async fn fetch<S: PackageSource>(
    source: &S,
    id: &PackageId,
    version: Option<PackageVersion>,
    requested_by: &Requester,
) -> Result<SourceVersion, ResolveError> {
    let result = match version {
        Some(version) => {
            source
                .exact(&PackageRelease {
                    id: id.clone(),
                    version,
                })
                .await
        }
        None => source.latest(id).await,
    };
    result.map_err(|cause| {
        ResolveError::Unknown(Box::new(UnknownPackage {
            id: id.clone(),
            version,
            requested_by: requested_by.clone(),
            cause,
        }))
    })
}

fn enqueue_dependencies(
    queue: &mut Vec<Want>,
    id: &PackageId,
    found: &SourceVersion,
    depth: usize,
) {
    let parent = PackageRelease {
        id: id.clone(),
        version: found.version,
    };
    for dependency in &found.dependencies {
        queue.push(Want {
            id: dependency.id.clone(),
            version: dependency.version,
            depth: depth + 1,
            requested_by: Requester::Dependency(parent.clone()),
        });
    }
}

#[cfg(test)]
#[path = "tests/resolve.rs"]
mod tests;
