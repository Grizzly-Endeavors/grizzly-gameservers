//! Whole-`/data` archive: streaming create and extract of the instance's data
//! root as a zstd-compressed tar, plus the pre-restore purge.
//!
//! Unlike the per-file [`crate::fs`] routes (small config edits, capped at
//! kilobytes), this moves the entire world — potentially gigabytes — so it
//! streams through the system `tar` rather than buffering. `tar` is used
//! deliberately: it is battle-tested against the symlinks, permissions, and
//! large files a real game data tree carries, where a hand-rolled walker would be
//! a liability. Extraction is confined to the data root by running `tar -C` there
//! (GNU tar strips a leading `/` and skips `..` members that would escape), and
//! the purge only ever removes the data root's own direct children.

use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result};
use tokio::process::{Child, Command};

const PRESERVED_ON_PURGE: &[&str] = &["lost+found"];

pub fn spawn_create(root: &Path) -> Result<Child> {
    let mut cmd = Command::new("tar");
    cmd.arg("--zstd")
        .arg("-cf")
        .arg("-")
        .arg("-C")
        .arg(root)
        .arg(".");
    cmd.kill_on_drop(true);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.spawn()
        .context("failed to spawn tar to create the data archive")
}

pub fn spawn_extract(root: &Path) -> Result<Child> {
    let mut cmd = Command::new("tar");
    cmd.arg("--zstd").arg("-xf").arg("-").arg("-C").arg(root);
    cmd.kill_on_drop(true);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::piped());
    cmd.spawn()
        .context("failed to spawn tar to extract the data archive")
}

pub fn purge(root: &Path) -> Result<()> {
    for entry in std::fs::read_dir(root)
        .with_context(|| format!("failed to read data root {} for purge", root.display()))?
    {
        let entry = entry.context("failed to read a data-root entry during purge")?;
        let name = entry.file_name();
        if PRESERVED_ON_PURGE
            .iter()
            .any(|preserved| name == *preserved)
        {
            continue;
        }
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to stat {} during purge", path.display()))?;
        let result = if file_type.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        result.with_context(|| format!("failed to remove {} during purge", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/archive.rs"]
mod tests;
