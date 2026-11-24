use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};

const MAGIC: &str = "#r2modman";

const MANIFEST: &str = "export.r2x";

const DEFAULT_PROFILE_NAME: &str = "Default";

/// Largest payload accepted from the registry, before any decoding. The verified
/// samples were 4.5 KB and 442 bytes; Thunderstore publishes no limit of its own,
/// so this is ours, and it is checked before the base64 rather than after so a
/// hostile blob is never expanded.
pub(crate) const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;

const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

pub(crate) const MAX_PACKAGES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProfileMod {
    pub(crate) full_name: String,
    pub(crate) version: String,
    /// r2modman lets a player keep a mod in a profile without running it.
    pub(crate) enabled: bool,
}

impl ProfileMod {
    pub(crate) fn pinned_name(&self) -> String {
        format!("{}-{}", self.full_name, self.version)
    }
}

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
    entry
        .by_ref()
        .take(MAX_MANIFEST_BYTES)
        .read_to_string(&mut manifest)
        .with_context(|| format!("profile's {MANIFEST} is not readable text"))?;
    Ok(manifest)
}

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
