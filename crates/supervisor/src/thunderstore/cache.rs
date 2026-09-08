//! The on-disk zip cache: `<work dir>/cache/<Namespace-Name-1.2.3>.zip`.
//!
//! The cache is not a performance optimisation, it is the offline story. Every
//! zip carries its own `manifest.json` with the same dependency strings the API
//! reports, so a pinned mod list resolves and installs from cache alone — which
//! is what lets a pod recreated during a Thunderstore outage still come back up
//! with the mods it had. The filename is the release string precisely so the
//! directory can be read back as a set of releases with no index file to fall
//! out of sync.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::types::PackageRelease;

/// Subdirectory of the working tree that holds the zips.
pub const CACHE_DIR: &str = "cache";

/// Suffix a download carries until it is complete. A `.part` file is never a
/// cache hit, so an interrupted download can't be mistaken for a package.
const PARTIAL_SUFFIX: &str = ".part";

/// Where one release's zip lives.
#[must_use]
pub fn path_for(work_dir: &Path, release: &PackageRelease) -> PathBuf {
    work_dir.join(CACHE_DIR).join(format!("{release}.zip"))
}

/// Open a cached zip, or `None` when it isn't cached.
///
/// # Errors
///
/// Returns an error if the file exists but can't be opened.
pub fn open(work_dir: &Path, release: &PackageRelease) -> Result<Option<std::fs::File>> {
    let path = path_for(work_dir, release);
    match std::fs::File::open(&path) {
        Ok(file) => Ok(Some(file)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to open {}", path.display())),
    }
}

/// Write `bytes` into the cache atomically: a temporary `.part` file renamed
/// into place, so a crash or a killed pod mid-download leaves no zip that looks
/// complete.
///
/// # Errors
///
/// Returns an error if the cache directory can't be created or the file can't be
/// written or renamed.
pub fn store(work_dir: &Path, release: &PackageRelease, bytes: &[u8]) -> Result<()> {
    let final_path = path_for(work_dir, release);
    let dir = final_path
        .parent()
        .context("cache path has no parent directory")?;
    std::fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let partial = final_path.with_extension(format!("zip{PARTIAL_SUFFIX}"));
    std::fs::write(&partial, bytes)
        .with_context(|| format!("failed to write {}", partial.display()))?;
    std::fs::rename(&partial, &final_path)
        .with_context(|| format!("failed to move {} into place", partial.display()))?;
    Ok(())
}

/// Every release currently cached. Unreadable or unparseable filenames are
/// ignored rather than fatal: the cache is derived state, and a stray file in it
/// must not stop a launch.
#[must_use]
pub fn cached_releases(work_dir: &Path) -> Vec<PackageRelease> {
    let Ok(entries) = std::fs::read_dir(work_dir.join(CACHE_DIR)) else {
        return Vec::new();
    };
    entries
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            name.strip_suffix(".zip")?.parse::<PackageRelease>().ok()
        })
        .collect()
}

/// Delete every cached zip outside `keep`, plus any leftover partial downloads.
/// Returns how many files were removed.
///
/// # Errors
///
/// Returns an error if the cache directory exists but can't be read. A file that
/// can't be deleted is left in place — the next launch will try again, and a
/// stale zip costs disk, not correctness.
pub fn collect(work_dir: &Path, keep: &[PackageRelease]) -> Result<usize> {
    let dir = work_dir.join(CACHE_DIR);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", dir.display())),
    };
    let mut removed = 0;
    for entry in entries.filter_map(std::result::Result::ok) {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let wanted = name
            .strip_suffix(".zip")
            .and_then(|stem| stem.parse::<PackageRelease>().ok())
            .is_some_and(|release| keep.contains(&release));
        if wanted {
            continue;
        }
        if std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
#[path = "tests/cache.rs"]
mod tests;
