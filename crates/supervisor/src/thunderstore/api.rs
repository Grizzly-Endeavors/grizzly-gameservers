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

const METADATA_TIMEOUT: Duration = Duration::from_secs(15);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_mins(3);

/// An identifying User-Agent. Thunderstore publishes no rate limit, so naming
/// this project is what lets them contact us rather than block us if these calls
/// ever become a problem.
const AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Deserialize)]
struct VersionBody {
    version_number: String,
    #[serde(default)]
    dependencies: Vec<String>,
}

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
    pub fn new() -> Result<Self> {
        Ok(Self {
            http: Client::builder()
                .timeout(DOWNLOAD_TIMEOUT)
                .build()
                .context("failed to build the thunderstore http client")?,
        })
    }

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

pub struct LayeredSource<'a, S: PackageSource + Sync> {
    registry: &'a S,
    work_dir: PathBuf,
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

    fn cached(&self, release: &PackageRelease) -> Option<SourceVersion> {
        let file = cache::open(&self.work_dir, release).ok()??;
        let manifest = super::archive::read_manifest(file, release).ok()?;
        Some(SourceVersion {
            version: release.version,
            dependencies: parse_dependencies(&manifest.dependencies),
        })
    }

    fn highest_cached(&self, id: &PackageId) -> Option<PackageVersion> {
        cache::cached_releases(&self.work_dir)
            .into_iter()
            .filter(|release| release.id == *id)
            .map(|release| release.version)
            .max()
    }
}

fn parse_dependencies(raw: &[String]) -> Vec<PackageRelease> {
    raw.iter()
        .filter_map(|entry| entry.parse::<PackageRelease>().ok())
        .collect()
}

impl<S: PackageSource + Sync> PackageSource for LayeredSource<'_, S> {
    async fn latest(&self, id: &PackageId) -> Result<SourceVersion, SourceError> {
        match self.registry.latest(id).await {
            Ok(found) => Ok(found),
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
