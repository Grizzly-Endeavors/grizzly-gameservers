use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::types::PackageRelease;

pub const CACHE_DIR: &str = "cache";

const PARTIAL_SUFFIX: &str = ".part";

#[must_use]
pub fn path_for(work_dir: &Path, release: &PackageRelease) -> PathBuf {
    work_dir.join(CACHE_DIR).join(format!("{release}.zip"))
}

pub fn open(work_dir: &Path, release: &PackageRelease) -> Result<Option<std::fs::File>> {
    let path = path_for(work_dir, release);
    match std::fs::File::open(&path) {
        Ok(file) => Ok(Some(file)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to open {}", path.display())),
    }
}

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
