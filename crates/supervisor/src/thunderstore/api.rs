//! The IO half of resolution: the Thunderstore HTTP client, and the layered
//! source that puts it in front of (or behind) the on-disk cache.
//!
//! The layering is not uniform, and the asymmetry is the whole point:
//!
//! - An **exact** version is looked up in the cache first. A cached zip carries
//!   the same dependency strings the API would report, so a fully pinned mod
//!   list resolves with no network at all — which is what makes a pod recreated
//!   during a Thunderstore outage come back with the mods it had.
//! - The **latest** version can only come from the API. When it is unreachable,
//!   the highest cached version stands in and the launch warns, because coming
//!   back up one version behind beats not coming back up.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::{Client, StatusCode, header::USER_AGENT};
use serde::Deserialize;

use super::archive::MAX_ARCHIVE_BYTES;
use super::cache;
use super::resolve::{PackageSource, SourceError, SourceVersion};
use super::types::{PackageId, PackageRelease, PackageVersion};

const API_BASE: &str = "https://thunderstore.io/api/experimental/package";

/// Per-request ceiling for a metadata lookup. Downloads get their own, longer
/// budget — a 200 MB texture pack is a slow request, not a stuck one.
const METADATA_TIMEOUT: Duration = Duration::from_secs(15);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_mins(3);

/// An identifying User-Agent. Thunderstore publishes no rate limit, so naming
/// this project is what lets them contact us rather than block us if these calls
/// ever become a problem.
const AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

/// The subset of a package-version response this crate reads.
#[derive(Debug, Deserialize)]
struct VersionBody {
    version_number: String,
    #[serde(default)]
    dependencies: Vec<String>,
}

/// The package response, whose `latest` is a [`VersionBody`].
#[derive(Debug, Deserialize)]
struct PackageBody {
    latest: VersionBody,
}

/// Thunderstore's read API. Holds a client of its own rather than borrowing the
/// supervisor's: the SDK client's timeout is sized for loopback calls to the
/// Agones sidecar and would abort every one of these.
pub struct ThunderstoreApi {
    http: Client,
}

impl ThunderstoreApi {
    /// Build a client for the read API.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client can't be built.
    pub fn new() -> Result<Self> {
        Ok(Self {
            http: Client::builder()
                .timeout(DOWNLOAD_TIMEOUT)
                .build()
                .context("failed to build the thunderstore http client")?,
        })
    }

    /// The package endpoint, whose body wraps the newest version.
    async fn get_version_of_package(&self, url: &str) -> Result<VersionBody, SourceError> {
        let response = self
            .http
            .get(url)
            .header(USER_AGENT, AGENT)
            .timeout(METADATA_TIMEOUT)
            .send()
            .await
            .map_err(|err| SourceError::Unavailable(err.to_string()))?;
        match response.status() {
            StatusCode::NOT_FOUND => Err(SourceError::NotFound),
            status if status.is_success() => response
                .json::<PackageBody>()
                .await
                .map(|body| body.latest)
                .map_err(|err| SourceError::Unavailable(format!("unreadable reply: {err}"))),
            status => Err(SourceError::Unavailable(format!("http {status}"))),
        }
    }

    async fn get_version(&self, url: &str) -> Result<VersionBody, SourceError> {
        let response = self
            .http
            .get(url)
            .header(USER_AGENT, AGENT)
            .timeout(METADATA_TIMEOUT)
            .send()
            .await
            .map_err(|err| SourceError::Unavailable(err.to_string()))?;
        match response.status() {
            StatusCode::NOT_FOUND => Err(SourceError::NotFound),
            status if status.is_success() => response
                .json::<VersionBody>()
                .await
                .map_err(|err| SourceError::Unavailable(format!("unreadable reply: {err}"))),
            status => Err(SourceError::Unavailable(format!("http {status}"))),
        }
    }

    /// Download one release's zip, bounded by [`MAX_ARCHIVE_BYTES`] on both the
    /// declared length and the bytes actually delivered — a lying or absent
    /// `Content-Length` must not be the only thing standing between a bad
    /// package and a full PVC.
    ///
    /// # Errors
    ///
    /// Returns an error if the download fails, is refused, or exceeds the cap.
    pub async fn download(&self, release: &PackageRelease) -> Result<Vec<u8>> {
        let url = format!(
            "https://thunderstore.io/package/download/{}/{}/{}/",
            release.id.namespace, release.id.name, release.version
        );
        let mut response = self
            .http
            .get(&url)
            .header(USER_AGENT, AGENT)
            .send()
            .await
            .with_context(|| format!("failed to download {release}"))?
            .error_for_status()
            .with_context(|| format!("failed to download {release}"))?;
        if let Some(length) = response.content_length()
            && length > MAX_ARCHIVE_BYTES
        {
            bail!("{release} is {length} bytes, over the {MAX_ARCHIVE_BYTES} byte limit");
        }
        let mut bytes: Vec<u8> = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .with_context(|| format!("failed while downloading {release}"))?
        {
            bytes.extend_from_slice(&chunk);
            if bytes.len() as u64 > MAX_ARCHIVE_BYTES {
                bail!("{release} exceeded the {MAX_ARCHIVE_BYTES} byte limit while downloading");
            }
        }
        Ok(bytes)
    }
}

/// The registry answering for itself, with no cache in front of it. Wrapped by
/// [`LayeredSource`] in production; implemented here so the caching layer can be
/// tested over a fake registry instead of the real one.
impl PackageSource for ThunderstoreApi {
    async fn latest(&self, id: &PackageId) -> Result<SourceVersion, SourceError> {
        let url = format!("{API_BASE}/{}/{}/", id.namespace, id.name);
        let body = self.get_version_of_package(&url).await?;
        let version = body
            .version_number
            .parse::<PackageVersion>()
            .map_err(|err| SourceError::Unavailable(format!("bad version: {err}")))?;
        Ok(SourceVersion {
            version,
            dependencies: parse_dependencies(&body.dependencies),
        })
    }

    async fn exact(&self, release: &PackageRelease) -> Result<SourceVersion, SourceError> {
        let url = format!(
            "{API_BASE}/{}/{}/{}/",
            release.id.namespace, release.id.name, release.version
        );
        let body = self.get_version(&url).await?;
        Ok(SourceVersion {
            version: release.version,
            dependencies: parse_dependencies(&body.dependencies),
        })
    }
}

/// The [`PackageSource`] the reconciler resolves through: a registry layered
/// over the on-disk cache, per the module docs.
pub struct LayeredSource<'a, S: PackageSource + Sync> {
    registry: &'a S,
    work_dir: PathBuf,
    /// Degradations worth reporting, collected because the trait's methods take
    /// `&self` and the caller wants them once resolution is over.
    notes: Mutex<Vec<String>>,
}

impl<'a, S: PackageSource + Sync> LayeredSource<'a, S> {
    #[must_use]
    pub fn new(registry: &'a S, work_dir: &Path) -> Self {
        Self {
            registry,
            work_dir: work_dir.to_owned(),
            notes: Mutex::new(Vec::new()),
        }
    }

    /// Everything that had to be degraded during resolution, in order.
    #[must_use]
    pub fn take_notes(&self) -> Vec<String> {
        let mut notes = self
            .notes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut *notes)
    }

    fn note(&self, message: String) {
        self.notes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(message);
    }

    /// Read one release's dependencies out of its cached zip, or `None` when it
    /// isn't cached or the zip doesn't hold up.
    fn cached(&self, release: &PackageRelease) -> Option<SourceVersion> {
        let file = cache::open(&self.work_dir, release).ok()??;
        let manifest = super::archive::read_manifest(file, release).ok()?;
        Some(SourceVersion {
            version: release.version,
            dependencies: parse_dependencies(&manifest.dependencies),
        })
    }

    /// The highest cached version of `id`, used when the API can't answer a
    /// "newest" question.
    fn highest_cached(&self, id: &PackageId) -> Option<PackageVersion> {
        cache::cached_releases(&self.work_dir)
            .into_iter()
            .filter(|release| release.id == *id)
            .map(|release| release.version)
            .max()
    }
}

/// Turn Thunderstore's `Namespace-Name-1.2.3` dependency strings into releases,
/// dropping any this crate can't parse.
///
/// Dropping rather than failing is deliberate: an unparseable dependency string
/// is upstream's problem with one entry, and refusing the whole launch over it
/// would make a third party able to break a running server by publishing a
/// malformed manifest.
fn parse_dependencies(raw: &[String]) -> Vec<PackageRelease> {
    raw.iter()
        .filter_map(|entry| entry.parse::<PackageRelease>().ok())
        .collect()
}

impl<S: PackageSource + Sync> PackageSource for LayeredSource<'_, S> {
    async fn latest(&self, id: &PackageId) -> Result<SourceVersion, SourceError> {
        match self.registry.latest(id).await {
            Ok(found) => Ok(found),
            // A package the registry says doesn't exist is a mistake in the mod
            // list, not an outage — falling back to a cached copy would install
            // something nobody can look up.
            Err(SourceError::NotFound) => Err(SourceError::NotFound),
            Err(SourceError::Unavailable(detail)) => {
                let Some(version) = self.highest_cached(id) else {
                    return Err(SourceError::Unavailable(detail));
                };
                let release = PackageRelease {
                    id: id.clone(),
                    version,
                };
                let Some(found) = self.cached(&release) else {
                    return Err(SourceError::Unavailable(detail));
                };
                self.note(format!(
                    "{id} is unpinned and the registry is unreachable; using the cached {version}"
                ));
                Ok(found)
            }
        }
    }

    async fn exact(&self, release: &PackageRelease) -> Result<SourceVersion, SourceError> {
        if let Some(found) = self.cached(release) {
            return Ok(found);
        }
        self.registry.exact(release).await
    }
}

#[cfg(test)]
#[path = "tests/api.rs"]
mod tests;
