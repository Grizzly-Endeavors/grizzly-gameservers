//! Pre-launch mod reconciliation for the Thunderstore/r2modman family of games.
//!
//! One declarative file on the instance's volume — `.grizzly/mods`, one package
//! per line — is turned into an installed set of packages before every child
//! launch. Nothing else in the pod installs mods; nothing here knows what game
//! it is running.
//!
//! **The file is the group's modpack, not this server's install list.** An entry
//! prefixed `client:` belongs on the players' machines, so this server installs
//! nothing for it and never resolves it; it is carried through to `resolved`
//! marked as such, which is what lets the bot mint a profile code describing the
//! whole pack rather than only the half that runs here.
//!
//! **Why the supervisor and not the bot.** The install has to inherit the
//! platform's existing safety property: a config change is snapshotted, applied,
//! verified, and rolled back automatically when the server doesn't come back.
//! Editing one text file through the ordinary file tools gets all of that for
//! free. A bot that downloaded packages and pushed them into the pod would put
//! the installed state *outside* that loop, and would put dependency resolution
//! inside an LLM turn.
//!
//! **Why staged rather than installed.** This module writes only into its own
//! working tree (`.grizzly/thunderstore/`), and the game image's own pre-launch
//! hook mirrors the staged tree into wherever that game's loader actually reads
//! from. That is the seam that keeps this code game-agnostic: adding the next
//! r2modman game is three environment variables and a mirror step in its image,
//! not a branch in here.
//!
//! **Why a failure stops the launch.** A mod list that names a package that
//! doesn't exist is a mistake somebody just made, and the recovery for it
//! already exists — the supervisor reports the child as `Crashed`, the bot
//! restores the file's snapshot and restarts. Degrading to "install what worked"
//! would instead leave a server running a set nobody asked for, with the wrong
//! answer written down as if it were right.

pub mod api;
pub mod archive;
pub mod cache;
pub mod mods_file;
pub mod resolve;
pub mod stage;
pub mod types;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tracing::{info, warn};

use crate::config::{SupervisorConfig, ThunderstoreMods};
use crate::logs::LogBuffer;

use api::{LayeredSource, ThunderstoreApi};
use resolve::{ResolvedPackage, Wanted};
use types::PackageRelease;

/// How long the whole reconcile may take before the launch gives up. Sized for a
/// cold instance pulling a handful of packages over the public internet; well
/// inside the bot's five-minute readiness ceiling, so a stuck registry surfaces
/// as a named failure the bot can roll back rather than as a timeout upstream.
const DEADLINE: Duration = Duration::from_mins(3);

/// Name of the file listing what actually got installed, written into the
/// working tree beside the cache. Plain data: Gary reads it back to confirm an
/// install rather than being told to trust the log, and the bot builds an
/// exported profile code from it — which is why its name and its shape are both
/// contract rather than local convention.
const RESOLVED_FILE: &str = grizzly_control_api::mods::RESOLVED_FILE;

/// Prefix on every line this subsystem pushes into the child-output buffer, so
/// `read_logs` shows mod activity in the same stream as the game's own output —
/// the one place the agent already looks after a restart.
const LOG_PREFIX: &str = "[mods]";

/// Reconcile the instance's mod list, leaving a staged install tree for the
/// image's pre-launch hook to mirror.
///
/// A no-op returning `Ok(())` when the game hasn't opted in.
///
/// # Errors
///
/// Returns an error if the mod list is malformed, a package or version doesn't
/// exist, a download fails with nothing cached to fall back on, an archive fails
/// validation, or the staged tree can't be built. Every one of these fails the
/// launch on purpose — see the module docs.
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

/// Seed the mod list, resolve it, fetch what's missing, and rebuild the staged
/// tree. Split from [`prepare`] so the deadline wraps the whole sequence rather
/// than being threaded through each step.
async fn reconcile(
    cfg: &SupervisorConfig,
    mods: &ThunderstoreMods,
    logs: &LogBuffer,
) -> Result<()> {
    let list_path = seed_mods_file(&cfg.data_dir, &mods.file)?;
    let work_dir = crate::fs::resolve_in_root(&cfg.data_dir, &mods.work_dir)
        .map_err(|err| anyhow::anyhow!("{err}"))
        .with_context(|| {
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

    // Pushed before anything is fetched: a dropped client line changes nothing
    // about this launch, so the one place it can be noticed is the log stream
    // the agent reads back afterwards.
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
    // Sorted for the same reason the installed set is: `resolved` is read back
    // and fingerprinted, so its order has to depend on the set and not on the
    // order somebody happened to type the lines in.
    client_only.sort();
    write_resolved(&work_dir, &keep, &client_only, &resolution.provided)?;
    match cache::collect(&work_dir, &keep) {
        Ok(removed) if removed > 0 => info!(removed, "collected unused mod archives"),
        Ok(_) => {}
        // The cache is derived state: failing to tidy it costs disk, not
        // correctness, so it must not fail a launch that otherwise worked.
        Err(err) => warn!(error = ?err, "could not collect unused mod archives"),
    }

    report(logs, &keep, &client_only, &resolution.provided, installed);
    Ok(())
}

/// Create the mod list if it isn't there, so the friend's very first edit is an
/// overwrite the bot can snapshot and roll back — the same reason the env
/// overlay is seeded.
fn seed_mods_file(root: &Path, rel: &str) -> Result<PathBuf> {
    crate::fs::seed_empty_file(root, rel)
        .map_err(|err| anyhow::anyhow!("{err}"))
        .with_context(|| format!("failed to seed the mod list {rel}"))
}

/// Fetch every resolved package that isn't already cached, validating each zip
/// against the release it claims to be before it is stored.
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
        // Checked before it lands in the cache: a zip that isn't what it claims
        // to be must never become a cache hit that later resolutions trust.
        archive::read_manifest(std::io::Cursor::new(&bytes), release).with_context(|| {
            format!("{release} did not download as the package it claims to be")
        })?;
        cache::store(work_dir, release, &bytes)?;
    }
    Ok(())
}

/// Whether the cache already holds a usable copy of `release`.
///
/// A cache hit has to *hold up*, not merely exist. Treating presence as proof
/// would make a damaged zip permanent: the download would be skipped, staging
/// would fail on it, and every restart afterwards would fail identically with no
/// way out short of someone deleting the file by hand.
///
/// # Errors
///
/// Returns an error only if the file exists and cannot be opened at all.
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

/// Write the `resolved` file: exactly what is installed, plus the two things
/// that are deliberately *not* — the packages the modpack carries for players'
/// machines, and the ones the game image supplies.
///
/// The whole modpack rather than just this server's share, because this file is
/// the single artifact an export is built from: a client-only entry that only
/// existed in `.grizzly/mods` would be dropped from every profile code minted
/// off a running server, which is exactly the entry a player most needs.
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

/// Push the one-line summary the agent reads back after a restart. Package names
/// only — never file contents — so a mod's own data can't reach the log stream.
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
        // Named rather than counted: this is the line that tells the agent a
        // package in the modpack was deliberately not installed here, and a bare
        // count would leave it unable to say which.
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

/// Comma-joined release names for one log line.
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
