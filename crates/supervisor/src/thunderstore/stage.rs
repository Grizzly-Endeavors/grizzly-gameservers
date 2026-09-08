//! The staged install tree: `<work dir>/staged/BepInEx/…`, rebuilt from scratch
//! on every launch and swapped into place when it is complete.
//!
//! Rebuilt rather than reconciled, because the mod list is the whole truth:
//! removing a line has to remove the files, and the only way to be sure of that
//! without tracking what each package installed is to build the tree from
//! nothing every time. Swapped rather than built in place, because a launch that
//! dies half-way through extraction would otherwise leave a tree that is neither
//! the old set nor the new one — and the image mirrors whatever it finds.
//!
//! The supervisor never writes into the live loader directory itself. It stages,
//! and the image's own pre-launch hook mirrors the staged tree across. That split
//! is what keeps this module free of any knowledge about where a given game
//! keeps `BepInEx`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::archive::{PackageLayout, extract};
use super::cache;
use super::resolve::ResolvedPackage;

/// Subdirectory holding the tree the image mirrors.
pub const STAGED_DIR: &str = "staged";

/// Subdirectory the next tree is built in before the swap.
const NEXT_DIR: &str = "staged.next";

/// The staged tree's path.
#[must_use]
pub fn staged_path(work_dir: &Path) -> PathBuf {
    work_dir.join(STAGED_DIR)
}

/// Build the staged tree for `packages` and swap it into place. Returns how many
/// files were installed.
///
/// # Errors
///
/// Returns an error if a package's zip is missing from the cache, an archive
/// fails validation, or the tree can't be built or swapped.
pub fn rebuild(
    work_dir: &Path,
    packages: &[ResolvedPackage],
    layout: PackageLayout,
) -> Result<usize> {
    let next = work_dir.join(NEXT_DIR);
    let staged = staged_path(work_dir);
    remove_tree(&next)?;
    std::fs::create_dir_all(&next)
        .with_context(|| format!("failed to create {}", next.display()))?;

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

/// Delete a directory tree, treating "already gone" as success.
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
