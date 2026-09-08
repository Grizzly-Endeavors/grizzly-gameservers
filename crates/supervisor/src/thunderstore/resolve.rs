//! Transitive dependency resolution over a [`PackageSource`].
//!
//! Pure in the sense that matters: it performs no IO of its own and holds no
//! filesystem or network knowledge — every fact it needs arrives through the
//! trait, so the whole rule set is exercised against a fake source.
//!
//! The rules, in the order they bite:
//!
//! - **An explicit pin is hard.** Nothing raises it. Thunderstore manifests
//!   declare what a package was *built against*, not what it requires, so a
//!   dependency asking for something newer than a pin is a preference, not a
//!   constraint — it warns and the pin stands.
//! - **A dependency's version is a floor.** The package is taken at exactly that
//!   version unless something else asks for a higher one.
//! - **The highest version wins** when several dependents disagree.
//! - **Provided packages are skipped by name.** The image supplies the loader
//!   pack; installing a second copy over it is how a working loader stops
//!   loading.
//! - **Bounded and cycle-safe.** A package is only re-resolved when the version
//!   strictly increases, so a dependency cycle settles instead of spinning; the
//!   package and depth caps bound the pathological cases that aren't cycles.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::future::Future;

use super::types::{PackageId, PackageRelease, PackageVersion};

/// Most packages one instance may end up with. A Valheim modpack runs to dozens;
/// a few hundred means something has gone wrong, and the cap turns "gone wrong"
/// into a named failure instead of a launch that downloads for an hour.
pub const MAX_PACKAGES: usize = 256;

/// Deepest dependency chain followed. Real chains are three or four deep.
pub const MAX_DEPTH: usize = 32;

/// One version of one package, as a source reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceVersion {
    pub version: PackageVersion,
    pub dependencies: Vec<PackageRelease>,
}

/// Why a source couldn't answer.
///
/// The split is load-bearing: `NotFound` is a mistake in the mod list that the
/// friend fixes by editing it, while `Unavailable` is the registry being down —
/// the same launch failure, but a completely different thing to tell them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceError {
    /// The registry answered, and has no such package or version.
    NotFound,
    /// The registry could not be asked, and no cached copy answered either.
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

/// Where a package came into the resolution — carried so every failure can point
/// at the line the friend wrote rather than at an id they've never seen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Requester {
    /// Listed directly, on this 1-based line of the mod list.
    Line(usize),
    /// Pulled in by another package.
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

/// Something worth telling the operator that did not stop the launch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveWarning {
    /// A dependency asked for a newer version than an explicit pin. The pin won.
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

/// Why resolution failed. Every variant names what was asked for and who asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    Unknown {
        id: PackageId,
        version: Option<PackageVersion>,
        requested_by: Requester,
        cause: SourceError,
    },
    TooManyPackages {
        limit: usize,
    },
    TooDeep {
        limit: usize,
        id: PackageId,
    },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown {
                id,
                version,
                requested_by,
                cause,
            } => {
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

/// Where version and dependency facts come from. Implemented over the live API
/// layered on the on-disk cache; the tests implement it over a map.
pub trait PackageSource {
    /// The newest version of `id`.
    fn latest(
        &self,
        id: &PackageId,
    ) -> impl Future<Output = Result<SourceVersion, SourceError>> + Send;

    /// One exact release.
    fn exact(
        &self,
        release: &PackageRelease,
    ) -> impl Future<Output = Result<SourceVersion, SourceError>> + Send;
}

/// One package the instance should end up with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPackage {
    pub release: PackageRelease,
    /// Whether the mod list pinned this version explicitly.
    pub pinned: bool,
}

/// Everything resolution produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resolution {
    /// What to install, ordered by package id so `resolved` is stable across runs.
    pub packages: Vec<ResolvedPackage>,
    /// Packages the image supplies, which were skipped rather than installed.
    pub provided: Vec<PackageId>,
    pub warnings: Vec<ResolveWarning>,
}

/// What the mod list asked for, before any resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wanted {
    pub id: PackageId,
    pub pin: Option<PackageVersion>,
    pub line_number: usize,
}

/// One pending dependency. Always carries an exact version — the mod list's own
/// entries are resolved up front, and a dependency string never says "newest".
struct Want {
    id: PackageId,
    version: PackageVersion,
    depth: usize,
    requested_by: Requester,
}

/// What has been decided about one package so far.
struct Decided {
    version: PackageVersion,
    pinned: bool,
}

/// Resolve `wanted` and everything it depends on.
///
/// # Errors
///
/// Returns a [`ResolveError`] if a package or version doesn't exist, the source
/// can't be reached, or the package/depth caps are exceeded.
pub async fn resolve<S: PackageSource>(
    source: &S,
    wanted: &[Wanted],
    provided: &[PackageId],
) -> Result<Resolution, ResolveError> {
    let mut decided: BTreeMap<PackageId, Decided> = BTreeMap::new();
    let mut skipped: BTreeSet<PackageId> = BTreeSet::new();
    let mut warnings = Vec::new();
    let mut queue: Vec<Want> = Vec::new();

    // Every explicitly listed package is resolved before any dependency is
    // followed. Order matters: a pinned package reached first as somebody else's
    // dependency would be recorded at the dependency's version, and the pin the
    // friend wrote would silently not apply.
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
            // Skipped whole, dependencies included: the image ships the pack as
            // a unit, and re-deriving its tree would install pieces of a second
            // copy over the one that works.
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
            // A dependency always names an exact version, so the standing
            // decision only moves when this one is strictly higher — which is
            // what makes a dependency cycle settle instead of spinning.
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

/// Ask the source for one package at one version (or the newest), mapping a
/// source failure into an error that names who asked for it.
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
    result.map_err(|cause| ResolveError::Unknown {
        id: id.clone(),
        version,
        requested_by: requested_by.clone(),
        cause,
    })
}

/// Queue everything `found` declares, attributed to the release that declared it.
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
