use std::io::{Read, Seek};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::types::{PackageId, PackageRelease, ZipManifest};

pub const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;

pub const MAX_ENTRY_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

pub const MAX_ENTRIES: usize = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    pub entries: usize,
    pub entry_bytes: u64,
    pub total_bytes: u64,
}

impl Caps {
    pub const DEFAULT: Self = Self {
        entries: MAX_ENTRIES,
        entry_bytes: MAX_ENTRY_BYTES,
        total_bytes: MAX_TOTAL_BYTES,
    };
}

const METADATA_FILES: [&str; 4] = ["manifest.json", "readme.md", "changelog.md", "icon.png"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageLayout {
    BepInEx,
}

impl PackageLayout {
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "bepinex" => Some(Self::BepInEx),
            _ => None,
        }
    }

    #[must_use]
    pub fn skeleton(self) -> &'static [&'static str] {
        match self {
            Self::BepInEx => &[
                "BepInEx/plugins",
                "BepInEx/patchers",
                "BepInEx/monomod",
                "BepInEx/config",
            ],
        }
    }
}

const PER_PACKAGE_DIRS: [&str; 3] = ["plugins", "patchers", "monomod"];

#[must_use]
pub fn plan_layout(layout: PackageLayout, id: &PackageId, path: &Path) -> Option<PathBuf> {
    let PackageLayout::BepInEx = layout;
    let components: Vec<&str> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir => None,
        })
        .collect();
    let [first, rest @ ..] = components.as_slice() else {
        return None;
    };

    let (head, tail) = if first.eq_ignore_ascii_case("BepInEx") {
        match rest {
            [head, tail @ ..] => (*head, tail),
            [] => return None,
        }
    } else {
        (*first, rest)
    };

    if tail.is_empty()
        && METADATA_FILES
            .iter()
            .any(|name| head.eq_ignore_ascii_case(name))
    {
        return None;
    }
    if head.eq_ignore_ascii_case("core") {
        return None;
    }
    if let Some(dir) = PER_PACKAGE_DIRS
        .iter()
        .find(|dir| head.eq_ignore_ascii_case(dir))
    {
        if tail.is_empty() {
            return None;
        }
        return Some(
            Path::new("BepInEx")
                .join(dir)
                .join(id.to_string())
                .join(tail.join("/")),
        );
    }
    if head.eq_ignore_ascii_case("config") {
        if tail.is_empty() {
            return None;
        }
        return Some(Path::new("BepInEx/config").join(tail.join("/")));
    }
    Some(
        Path::new("BepInEx/plugins")
            .join(id.to_string())
            .join(components.join("/")),
    )
}

/// A zip entry's name as this extractor will act on it: separators normalised,
/// and refused outright if it addresses anywhere but inside the tree.
///
/// A path that climbs is rejected rather than sanitised: an archive containing
/// `../evil.dll` is not a package with one bad path, it is an archive doing
/// something it has no reason to do, and the rest of its files are no more
/// trustworthy.
///
/// A **backslash is a separator here, not an attack.** The zip spec says entries
/// use `/` (APPNOTE 4.4.17.1), but Windows packers write `\` anyway and
/// r2modman installs those packages happily — so refusing them refused real,
/// correctly-built mods over their packaging tool. Normalising *before* the
/// checks below is also what makes those checks real: on Unix
/// `Path::new("..\\evil.dll")` is a single component literally named
/// `..\evil.dll`, so `ParentDir` would never match and the climb would pass. The
/// blanket rejection was carrying that weight; now the traversal check sees the
/// same path the extractor will write.
///
/// # Errors
///
/// Returns an error if the name holds a NUL byte, is absolute, or climbs out of
/// the archive. Messages quote the archive's own spelling, since what is being
/// judged is what it claims.
fn safe_entry_name(name: &str) -> Result<String> {
    if name.contains('\0') {
        bail!("archive entry {name:?} contains a NUL byte");
    }
    let normalized = name.replace('\\', "/");
    if normalized.starts_with('/') {
        bail!("archive entry {name:?} is an absolute path");
    }
    if Path::new(&normalized)
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
    {
        bail!("archive entry {name:?} climbs out of the archive");
    }
    Ok(normalized)
}

pub fn read_manifest<R: Read + Seek>(reader: R, expected: &PackageRelease) -> Result<ZipManifest> {
    let mut zip = zip::ZipArchive::new(reader).context("failed to open the package archive")?;
    let mut file = zip
        .by_name("manifest.json")
        .context("package archive has no manifest.json at its root")?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .context("failed to read manifest.json from the package archive")?;
    let manifest: ZipManifest =
        serde_json::from_str(strip_bom(&text)).context("failed to parse manifest.json")?;
    if manifest.name != expected.id.name {
        bail!(
            "package archive for {expected} contains {}, not {}",
            manifest.name,
            expected.id.name
        );
    }
    if manifest.version_number != expected.version.to_string() {
        bail!(
            "package archive for {expected} declares version {}",
            manifest.version_number
        );
    }
    Ok(manifest)
}

/// Drop a leading UTF-8 byte-order mark.
///
/// Thunderstore packages are authored on Windows more often than not, and a good
/// share of them ship a `manifest.json` that starts with `EF BB BF`. JSON has no
/// notion of a BOM, so `serde_json` refuses the document at column 1 — which
/// surfaced as `ValheimModding-Jotunn` failing validation, and would fail every
/// package written by the same tooling. Stripping it is what the rest of the
/// ecosystem does; refusing would mean refusing packages r2modman installs fine.
fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

pub fn extract<R: Read + Seek>(
    reader: R,
    release: &PackageRelease,
    layout: PackageLayout,
    dest: &Path,
) -> Result<usize> {
    extract_within(reader, release, layout, dest, Caps::DEFAULT)
}

pub fn extract_within<R: Read + Seek>(
    reader: R,
    release: &PackageRelease,
    layout: PackageLayout,
    dest: &Path,
    caps: Caps,
) -> Result<usize> {
    let mut zip = zip::ZipArchive::new(reader).context("failed to open the package archive")?;
    if zip.len() > caps.entries {
        bail!(
            "package archive for {release} holds {} files, over the {} limit",
            zip.len(),
            caps.entries
        );
    }
    let mut total: u64 = 0;
    let mut installed = 0;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .with_context(|| format!("failed to read entry {index} of {release}"))?;
        let name = entry
            .name()
            .with_context(|| {
                format!("an entry name in the archive for {release} is not valid text")
            })?
            .into_owned();
        let entry_path =
            safe_entry_name(&name).with_context(|| format!("in the archive for {release}"))?;
        if entry.is_symlink() {
            bail!("archive entry {name:?} in {release} is a symlink");
        }
        if entry.is_dir() {
            continue;
        }
        if entry.size() > caps.entry_bytes {
            bail!(
                "archive entry {name:?} in {release} is larger than {} bytes",
                caps.entry_bytes
            );
        }
        total = total.saturating_add(entry.size());
        if total > caps.total_bytes {
            bail!(
                "the archive for {release} expands past {} bytes",
                caps.total_bytes
            );
        }
        let Some(relative) = plan_layout(layout, &release.id, Path::new(&entry_path)) else {
            continue;
        };
        let target = dest.join(&relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let mut out = std::fs::File::create(&target)
            .with_context(|| format!("failed to create {}", target.display()))?;
        std::io::copy(&mut entry, &mut out)
            .with_context(|| format!("failed to write {}", target.display()))?;
        installed += 1;
    }
    Ok(installed)
}

#[cfg(test)]
#[path = "tests/archive.rs"]
mod tests;
