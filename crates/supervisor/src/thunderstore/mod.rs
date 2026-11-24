pub mod api;
pub mod archive;
pub mod cache;
pub mod mods_file;
pub mod orphaned_config;
pub mod resolve;
pub mod stage;
pub mod types;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tracing::{error, info, warn};

use crate::config::{SupervisorConfig, ThunderstoreMods};
use crate::logs::LogBuffer;

use api::{LayeredSource, ThunderstoreApi};
use resolve::{ResolvedPackage, Wanted};
use types::PackageRelease;

const DEADLINE: Duration = Duration::from_mins(3);

const RESOLVED_FILE: &str = grizzly_control_api::mods::RESOLVED_FILE;

const LOG_PREFIX: &str = "[mods]";

pub async fn prepare(cfg: &SupervisorConfig, logs: &LogBuffer) -> Result<()> {
    let Some(mods) = cfg.thunderstore.as_ref() else {
        return Ok(());
    };
    match tokio::time::timeout(DEADLINE, reconcile(cfg, mods, logs)).await {
        Ok(result) => result,
        Err(_elapsed) => bail!(
            "mod install did not finish within {} seconds",
            DEADLINE.as_secs()
        ),
    }
}

async fn reconcile(
    cfg: &SupervisorConfig,
    mods: &ThunderstoreMods,
    logs: &LogBuffer,
) -> Result<()> {
    let list_path = seed_mods_file(&cfg.data_dir, &mods.file)?;
    let work_dir = in_root(&cfg.data_dir, &mods.work_dir).with_context(|| {
        format!(
            "mod working directory {} is not inside the data root",
            mods.work_dir
        )
    })?;
    std::fs::create_dir_all(&work_dir)
        .with_context(|| format!("failed to create {}", work_dir.display()))?;

    let text = std::fs::read_to_string(&list_path)
        .with_context(|| format!("failed to read {}", mods.file))?;
    let list = mods_file::parse(&text).with_context(|| format!("{} is unusable", mods.file))?;

    for warning in &list.warnings {
        warn!(warning = %warning, "mod list entry skipped");
        logs.push(format!("{LOG_PREFIX} {warning}"));
    }

    let api = ThunderstoreApi::new()?;
    let source = LayeredSource::new(&api, &work_dir);
    let wanted: Vec<Wanted> = list
        .install
        .into_iter()
        .map(|entry| Wanted {
            id: entry.id,
            pin: entry.pin,
            line_number: entry.line_number,
        })
        .collect();
    let resolution = resolve::resolve(&source, &wanted, &mods.provided)
        .await
        .with_context(|| format!("could not work out what {} asks for", mods.file))?;

    for note in source.take_notes() {
        warn!(note = %note, "mod resolution degraded");
        logs.push(format!("{LOG_PREFIX} {note}"));
    }
    for warning in &resolution.warnings {
        warn!(warning = %warning, "mod resolution warning");
        logs.push(format!("{LOG_PREFIX} {warning}"));
    }

    download_missing(&api, &work_dir, &resolution.packages).await?;
    let installed = stage::rebuild(&work_dir, &resolution.packages, mods.layout)?;

    let keep: Vec<PackageRelease> = resolution
        .packages
        .iter()
        .map(|package| package.release.clone())
        .collect();
    let mut client_only: Vec<PackageRelease> = list
        .client_only
        .into_iter()
        .map(|entry| entry.release)
        .collect();
    client_only.sort();
    write_resolved(&work_dir, &keep, &client_only, &resolution.provided)?;
    match cache::collect(&work_dir, &keep) {
        Ok(removed) if removed > 0 => info!(removed, "collected unused mod archives"),
        Ok(_) => {}
        Err(err) => warn!(error = ?err, "could not collect unused mod archives"),
    }

    report(logs, &keep, &client_only, &resolution.provided, installed);
    Ok(())
}

pub struct OrphanedConfigSweep {
    declared_dir: String,
    config_path: PathBuf,
    staged_path: PathBuf,
    keep: Vec<String>,
}

impl OrphanedConfigSweep {
    pub fn plan(cfg: &SupervisorConfig) -> Result<Option<Self>> {
        let Some(mods) = cfg.thunderstore.as_ref() else {
            return Ok(None);
        };
        let Some(declared_dir) = mods.config_dir.clone() else {
            return Ok(None);
        };
        let config_path = in_root(&cfg.data_dir, &declared_dir).with_context(|| {
            format!("mod settings directory {declared_dir} is not inside the data root")
        })?;
        let work_dir = in_root(&cfg.data_dir, &mods.work_dir).with_context(|| {
            format!(
                "mod working directory {} is not inside the data root",
                mods.work_dir
            )
        })?;
        Ok(Some(Self {
            declared_dir,
            config_path,
            staged_path: stage::staged_path(&work_dir),
            keep: mods.config_keep.clone(),
        }))
    }

    pub fn run(self, logs: &LogBuffer) {
        let report = match orphaned_config::sweep(&self.config_path, &self.staged_path, &self.keep)
        {
            Ok(report) => report,
            Err(err) => {
                error!(
                    error = ?err,
                    dir = %self.declared_dir,
                    "could not sweep the settings of uninstalled mods"
                );
                return;
            }
        };
        if report.removed.is_empty() {
            return;
        }
        let names = report
            .removed
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        logs.push(format!(
            "{LOG_PREFIX} removed settings for {} uninstalled mod(s) from {}: {names}",
            report.removed.len(),
            self.declared_dir
        ));
        info!(
            removed = report.removed.len(),
            kept = report.kept,
            "swept the settings of uninstalled mods"
        );
    }
}

fn in_root(root: &Path, rel: &str) -> Result<PathBuf> {
    crate::fs::resolve_in_root(root, rel).map_err(|err| anyhow::anyhow!("{err}"))
}

fn seed_mods_file(root: &Path, rel: &str) -> Result<PathBuf> {
    crate::fs::seed_empty_file(root, rel)
        .map_err(|err| anyhow::anyhow!("{err}"))
        .with_context(|| format!("failed to seed the mod list {rel}"))
}

async fn download_missing(
    api: &ThunderstoreApi,
    work_dir: &Path,
    packages: &[ResolvedPackage],
) -> Result<()> {
    for package in packages {
        let release = &package.release;
        if usable_in_cache(work_dir, release)? {
            continue;
        }
        info!(package = %release, "downloading mod");
        let bytes = api.download(release).await?;
        archive::read_manifest(std::io::Cursor::new(&bytes), release).with_context(|| {
            format!("{release} did not download as the package it claims to be")
        })?;
        cache::store(work_dir, release, &bytes)?;
    }
    Ok(())
}

fn usable_in_cache(work_dir: &Path, release: &PackageRelease) -> Result<bool> {
    let Some(cached) = cache::open(work_dir, release)? else {
        return Ok(false);
    };
    match archive::read_manifest(cached, release) {
        Ok(_) => Ok(true),
        Err(err) => {
            warn!(
                package = %release,
                error = ?err,
                "cached mod archive is unusable; fetching it again"
            );
            Ok(false)
        }
    }
}

fn write_resolved(
    work_dir: &Path,
    packages: &[PackageRelease],
    client_only: &[PackageRelease],
    provided: &[types::PackageId],
) -> Result<()> {
    let text = grizzly_control_api::mods::ResolvedMods {
        installed: packages.iter().map(ToString::to_string).collect(),
        client_only: client_only.iter().map(ToString::to_string).collect(),
        provided: provided.iter().map(ToString::to_string).collect(),
    }
    .render();
    let path = work_dir.join(RESOLVED_FILE);
    std::fs::write(&path, text).with_context(|| format!("failed to write {}", path.display()))
}

fn report(
    logs: &LogBuffer,
    packages: &[PackageRelease],
    client_only: &[PackageRelease],
    provided: &[types::PackageId],
    files: usize,
) {
    if packages.is_empty() {
        logs.push(format!("{LOG_PREFIX} no mods listed; nothing installed"));
    } else {
        let names = join(packages);
        logs.push(format!(
            "{LOG_PREFIX} installed {} package(s), {files} file(s): {names}",
            packages.len()
        ));
        info!(packages = %names, files, "mod install reconciled");
    }
    if !client_only.is_empty() {
        logs.push(format!(
            "{LOG_PREFIX} {} package(s) are for players' machines only: {}",
            client_only.len(),
            join(client_only)
        ));
    }
    if !provided.is_empty() {
        let skipped = provided
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        logs.push(format!(
            "{LOG_PREFIX} skipped {skipped} — supplied by the game image"
        ));
    }
}

fn join(releases: &[PackageRelease]) -> String {
    releases
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
#[path = "tests/thunderstore.rs"]
mod tests;
