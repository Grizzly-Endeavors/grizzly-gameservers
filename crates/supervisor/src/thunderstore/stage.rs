use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::archive::{PackageLayout, extract};
use super::cache;
use super::resolve::ResolvedPackage;

pub const STAGED_DIR: &str = "staged";

const NEXT_DIR: &str = "staged.next";

#[must_use]
pub fn staged_path(work_dir: &Path) -> PathBuf {
    work_dir.join(STAGED_DIR)
}

pub fn rebuild(
    work_dir: &Path,
    packages: &[ResolvedPackage],
    layout: PackageLayout,
) -> Result<usize> {
    let next = work_dir.join(NEXT_DIR);
    let staged = staged_path(work_dir);
    remove_tree(&next)?;
    for dir in layout.skeleton() {
        let path = next.join(dir);
        std::fs::create_dir_all(&path)
            .with_context(|| format!("failed to create {}", path.display()))?;
    }

    let mut installed = 0;
    for package in packages {
        let release = &package.release;
        let file = cache::open(work_dir, release)?.with_context(|| {
            format!("{release} is not in the cache; it should have been downloaded first")
        })?;
        installed += extract(file, release, layout, &next)
            .with_context(|| format!("failed to install {release}"))?;
    }

    remove_tree(&staged)?;
    std::fs::rename(&next, &staged).with_context(|| {
        format!(
            "failed to move {} into place at {}",
            next.display(),
            staged.display()
        )
    })?;
    Ok(installed)
}

fn remove_tree(path: &Path) -> Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("failed to remove {}", path.display())),
    }
}

#[cfg(test)]
#[path = "tests/stage.rs"]
mod tests;
