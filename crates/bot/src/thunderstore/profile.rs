//! r2modman profile payloads: the thing behind a profile code.
//!
//! A code is a UUID; the payload it names is the literal line `#r2modman`
//! followed by base64 of a zip, and the only part of that zip this crate reads
//! or writes is `export.r2x` — a YAML list of `Namespace-Name` packages with
//! their exact versions and an enabled flag.
//!
//! **Nothing in a payload is a mod file.** The zip a real export carries also
//! holds loader-level configuration (`config/BepInEx.cfg`, `doorstop_config.ini`)
//! that the game image owns and lloesche's updater manages, so this module
//! deliberately ignores it in both directions: an import must not write over the
//! loader, and an export of a set of names is exactly as trustworthy as the
//! registry the names are resolved from. That is what keeps the platform's "every
//! installed artifact comes from a moderated registry by name" property intact
//! across a feature whose input is a stranger's UUID.
//!
//! Pure: bytes in, bytes out, no IO. The fetching and minting live in
//! [`client`](super::client).

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};

/// First line of every payload, and the only version marker the format has.
const MAGIC: &str = "#r2modman";

/// The one member of the zip this module reads or writes.
const MANIFEST: &str = "export.r2x";

/// Profile name written into an export. r2modman shows it on the import screen;
/// the server it was minted from is the useful thing to say there, so callers
/// pass one rather than this being fixed.
const DEFAULT_PROFILE_NAME: &str = "Default";

/// Largest payload accepted from the registry, before any decoding. The verified
/// samples were 4.5 KB and 442 bytes; Thunderstore publishes no limit of its own,
/// so this is ours, and it is checked before the base64 rather than after so a
/// hostile blob is never expanded.
pub(crate) const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;

/// Largest `export.r2x` accepted out of the zip. A separate bound from the
/// payload's because base64 of a zip hides the expansion ratio: a small blob can
/// carry a very large member.
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

/// Most packages a profile may list. The same ceiling the supervisor's resolver
/// enforces (`resolve::MAX_PACKAGES`), applied here so an oversized profile is
/// refused while it is still a string, rather than after it has been written
/// into a mod list that then fails the server's next launch.
pub(crate) const MAX_PACKAGES: usize = 256;

/// One package a profile lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProfileMod {
    /// The `Namespace-Name` pair, exactly as the registry spells it.
    pub(crate) full_name: String,
    /// The three-part version, rendered.
    pub(crate) version: String,
    /// r2modman lets a player keep a mod in a profile without running it.
    pub(crate) enabled: bool,
}

impl ProfileMod {
    /// The `Namespace-Name-1.2.3` string a mod list and a `resolved` line use.
    pub(crate) fn pinned_name(&self) -> String {
        format!("{}-{}", self.full_name, self.version)
    }
}

/// Read a payload.
///
/// # Errors
///
/// Returns an error if the payload is too large, isn't an `#r2modman` blob,
/// isn't valid base64, isn't a zip, carries no `export.r2x`, doesn't parse as
/// the export format, or lists more than [`MAX_PACKAGES`] packages.
pub(crate) fn decode(payload: &[u8]) -> Result<Vec<ProfileMod>> {
    if payload.len() > MAX_PAYLOAD_BYTES {
        bail!(
            "profile payload is {} bytes, over the {MAX_PAYLOAD_BYTES}-byte limit",
            payload.len()
        );
    }
    let text = std::str::from_utf8(payload).context("profile payload is not text")?;
    let body = text
        .strip_prefix(MAGIC)
        .with_context(|| format!("profile payload does not start with {MAGIC}"))?;
    // Whitespace-tolerant rather than newline-exact: the marker is a line, and
    // how a server chooses to wrap the base64 after it is not part of the format.
    let encoded: String = body.split_whitespace().collect();
    let zipped = BASE64
        .decode(encoded)
        .context("profile payload is not valid base64")?;

    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(zipped)).context("profile is not a zip")?;
    let manifest = read_manifest(&mut archive)?;
    let export: Export =
        serde_saphyr::from_str(&manifest).context("profile's export.r2x is not readable")?;
    if export.mods.len() > MAX_PACKAGES {
        bail!(
            "profile lists {} packages, over the {MAX_PACKAGES}-package limit",
            export.mods.len()
        );
    }
    // Every name has to be a `Namespace-Name` pair before anything downstream
    // renders it. That grammar is alphanumerics, underscores and one dash — so a
    // name that passes it cannot carry a newline, a fence marker, or a megabyte
    // of prose into Gary's context, and one that fails could never have
    // installed anyway. Counted rather than quoted, for the same reason.
    let malformed = export
        .mods
        .iter()
        .filter(|entry| super::types::split_full_name(&entry.name).is_none())
        .count();
    if malformed > 0 {
        bail!("profile lists {malformed} entries whose names aren't Namespace-Name packages");
    }
    Ok(export.mods.into_iter().map(Into::into).collect())
}

/// Pull `export.r2x` out of the archive, bounded.
fn read_manifest(archive: &mut zip::ZipArchive<std::io::Cursor<Vec<u8>>>) -> Result<String> {
    use std::io::Read as _;

    let mut entry = archive
        .by_name(MANIFEST)
        .with_context(|| format!("profile has no {MANIFEST}"))?;
    if entry.size() > MAX_MANIFEST_BYTES {
        bail!(
            "profile's {MANIFEST} is {} bytes, over the {MAX_MANIFEST_BYTES}-byte limit",
            entry.size()
        );
    }
    let mut manifest = String::new();
    // Bounded on the way out as well as by the declared size: the header is the
    // archive's own claim about itself, and a zip is free to lie about it.
    entry
        .by_ref()
        .take(MAX_MANIFEST_BYTES)
        .read_to_string(&mut manifest)
        .with_context(|| format!("profile's {MANIFEST} is not readable text"))?;
    Ok(manifest)
}

/// Build a payload listing `mods`, with no configuration tree.
///
/// A zip carrying only `export.r2x` is a complete profile — verified by minting
/// one and fetching it back — and the configuration a real export carries is the
/// loader's, which belongs to the game image rather than to the group's modpack.
///
/// # Errors
///
/// Returns an error if the manifest can't be serialized or the zip can't be
/// built, neither of which depends on the input.
pub(crate) fn encode(profile_name: &str, mods: &[ProfileMod]) -> Result<Vec<u8>> {
    use std::io::Write as _;

    let name = if profile_name.trim().is_empty() {
        DEFAULT_PROFILE_NAME
    } else {
        profile_name
    };
    let export = Export {
        profile_name: name.to_owned(),
        mods: mods.iter().map(ExportMod::from).collect(),
    };
    let manifest = serde_saphyr::to_string(&export).context("failed to render export.r2x")?;

    let mut zipped = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zipped
        .start_file(
            MANIFEST,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .context("failed to start export.r2x in the profile zip")?;
    zipped
        .write_all(manifest.as_bytes())
        .context("failed to write export.r2x into the profile zip")?;
    let zipped = zipped
        .finish()
        .context("failed to finish the profile zip")?
        .into_inner();

    let mut payload = format!("{MAGIC}\n").into_bytes();
    payload.extend_from_slice(BASE64.encode(zipped).as_bytes());
    Ok(payload)
}

/// `export.r2x` itself. Field names are r2modman's and are not ours to rename.
#[derive(Debug, Deserialize, Serialize)]
struct Export {
    #[serde(rename = "profileName")]
    profile_name: String,
    mods: Vec<ExportMod>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ExportMod {
    name: String,
    version: ExportVersion,
    /// Absent in some hand-built exports; a mod nobody said to disable is on.
    #[serde(default = "enabled_by_default")]
    enabled: bool,
}

fn enabled_by_default() -> bool {
    true
}

/// r2modman splits a version into its three numeric parts rather than carrying
/// the string, which is why this type exists at all.
#[derive(Debug, Deserialize, Serialize)]
struct ExportVersion {
    major: u64,
    minor: u64,
    patch: u64,
}

impl From<ExportMod> for ProfileMod {
    fn from(entry: ExportMod) -> Self {
        Self {
            full_name: entry.name,
            version: format!(
                "{}.{}.{}",
                entry.version.major, entry.version.minor, entry.version.patch
            ),
            enabled: entry.enabled,
        }
    }
}

impl From<&ProfileMod> for ExportMod {
    fn from(entry: &ProfileMod) -> Self {
        let mut parts = entry
            .version
            .split('.')
            .map(|part| part.parse().unwrap_or(0));
        Self {
            name: entry.full_name.clone(),
            version: ExportVersion {
                // A version that isn't three numbers can't reach here: every
                // release this crate carries came out of the registry or out of a
                // `resolved` line, both of which are already that shape. Rendering
                // zeroes rather than failing keeps a single odd entry from
                // costing the whole export.
                major: parts.next().unwrap_or(0),
                minor: parts.next().unwrap_or(0),
                patch: parts.next().unwrap_or(0),
            },
            enabled: entry.enabled,
        }
    }
}

#[cfg(test)]
#[path = "tests/profile.rs"]
mod tests;
