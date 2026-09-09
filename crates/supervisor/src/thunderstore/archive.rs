//! Package zips: what a file inside one is allowed to be, and where it goes.
//!
//! Two jobs, kept together because neither is meaningful without the other:
//! rejecting an archive that would write outside the tree it is being extracted
//! into, and deciding the loader-relative path each surviving entry lands at.
//!
//! Package layouts vary wildly — some ship `plugins/Thing.dll`, some
//! `BepInEx/plugins/Thing.dll`, some a bare `Thing.dll` at the root — because
//! r2modman normalises them on install rather than the authors agreeing. The
//! normalisation rule is [`plan_layout`], and it is pure so the whole zoo is
//! covered by fixtures rather than by a live server.

use std::io::{Read, Seek};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::types::{PackageId, PackageRelease, ZipManifest};

/// Largest a single package zip may be, compressed. Generous for a texture pack,
/// small enough that a mistake can't fill the PVC.
pub const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;

/// Largest one entry may expand to, and the ceiling on a whole archive's
/// expanded size. Both are checked while streaming, so a zip bomb is stopped
/// part-way rather than after it has landed on disk.
pub const MAX_ENTRY_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

/// Most files one package may contain.
pub const MAX_ENTRIES: usize = 20_000;

/// The caps [`extract`] enforces while streaming an archive.
///
/// Carried as a value rather than read from the consts at each check so the
/// enforcement itself is testable: a fixture proving a 256 MiB entry is refused
/// would have to *be* 256 MiB, which is not a test anyone will run. The
/// production caps are [`Caps::DEFAULT`], and `extract` uses them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    pub entries: usize,
    pub entry_bytes: u64,
    pub total_bytes: u64,
}

impl Caps {
    /// What a real launch enforces.
    pub const DEFAULT: Self = Self {
        entries: MAX_ENTRIES,
        entry_bytes: MAX_ENTRY_BYTES,
        total_bytes: MAX_TOTAL_BYTES,
    };
}

/// Root files that describe the package rather than belonging to the game.
const METADATA_FILES: [&str; 4] = ["manifest.json", "readme.md", "changelog.md", "icon.png"];

/// Which per-game rule maps a package's files onto the loader's directories.
/// A closed enum: an unknown value in the image's environment is a hard error,
/// because the alternative is a game silently installing every mod into the
/// wrong place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageLayout {
    /// `BepInEx` — the r2modman rule the Thunderstore communities are built on.
    BepInEx,
}

impl PackageLayout {
    /// Parse the layout named in the image's environment.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "bepinex" => Some(Self::BepInEx),
            _ => None,
        }
    }

    /// The directories a staged tree always contains, whether or not any package
    /// puts anything in them.
    ///
    /// Load-bearing: the image's mirror step reads this tree as the *complete*
    /// desired state and deletes anything in the live loader that isn't in it.
    /// A tree that simply omitted the directories nobody installed into would
    /// make "no mods" indistinguishable from "no mod list at all" — and the
    /// mirror would skip, leaving the previous set installed. Removing the last
    /// mod has to leave an empty tree, not an absent one.
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

/// The directories a `BepInEx` package may address by name, each of which gets a
/// per-package subdirectory so removing a mod is removing one directory.
const PER_PACKAGE_DIRS: [&str; 3] = ["plugins", "patchers", "monomod"];

/// Where one file inside a package lands, relative to the loader root, or `None`
/// when it is not installed at all.
///
/// The rule, applied to the path with an optional leading `BepInEx/` stripped:
///
/// - `plugins`, `patchers`, `monomod` → `BepInEx/<dir>/<Namespace-Name>/<rest>`
/// - `config` → `BepInEx/config/<rest>`, flat, because plugins look their config
///   up by a fixed filename and a per-package subdirectory would hide it
/// - `core` → skipped: that is the loader itself, and a package that is not the
///   image's own loader pack must never overwrite it
/// - the four root metadata files → skipped
/// - anything else (a bare DLL, an asset folder) → `BepInEx/plugins/<Namespace-Name>/<path>`
///
/// Directory names are matched case-insensitively; packages spell them both ways.
#[must_use]
pub fn plan_layout(layout: PackageLayout, id: &PackageId, path: &Path) -> Option<PathBuf> {
    let PackageLayout::BepInEx = layout;
    let components: Vec<&str> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            // Everything else was already refused by `safe_entry_name`; a
            // caller passing one anyway gets it dropped rather than honoured.
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir => None,
        })
        .collect();
    let [first, rest @ ..] = components.as_slice() else {
        return None;
    };

    // A package may or may not wrap its tree in a BepInEx/ directory; both are
    // common, and after stripping it the two shapes are the same package.
    let (head, tail) = if first.eq_ignore_ascii_case("BepInEx") {
        match rest {
            [head, tail @ ..] => (*head, tail),
            // A bare `BepInEx/` entry with nothing under it addresses nothing.
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

/// Read and validate the `manifest.json` at the root of a package zip.
///
/// # Errors
///
/// Returns an error if the archive can't be read, has no root `manifest.json`,
/// or its manifest names a different package or version than `expected`. The
/// cross-check is what stops a mislabeled or swapped cache file installing under
/// the name the mod list asked for.
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

/// Extract one package into `dest`, laying its files out per `layout`.
///
/// `dest` is the staged loader root; every produced path is under it by
/// construction, because [`plan_layout`] builds from a fixed prefix and
/// [`safe_entry_name`] has already refused anything that could climb.
///
/// # Errors
///
/// Returns an error if the archive is unreadable, oversized, contains a symlink
/// or an unsafe entry name, or a file can't be written.
pub fn extract<R: Read + Seek>(
    reader: R,
    release: &PackageRelease,
    layout: PackageLayout,
    dest: &Path,
) -> Result<usize> {
    extract_within(reader, release, layout, dest, Caps::DEFAULT)
}

/// [`extract`] with the caps supplied, so the size guards can be exercised
/// against fixtures of a sane size. Production always passes [`Caps::DEFAULT`].
///
/// # Errors
///
/// As [`extract`].
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
        // The archive's own name, not `enclosed_name()`: that helper *sanitises*
        // a path, so an entry declaring `/etc/passwd` would arrive here already
        // rewritten to `etc/passwd` and sail through the check below. What has
        // to be judged is what the archive actually claims.
        let name = entry.name().to_owned();
        // The normalised form is what everything downstream must use: planning
        // the layout on the raw name would treat a Windows-packed `a\\b.dll` as
        // one filename and write a file with a backslash in its name.
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
