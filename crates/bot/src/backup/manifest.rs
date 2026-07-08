//! Pure helpers for the S3 backup/archive layout: the `manifest.json` sidecar
//! that makes every artifact self-describing, the object-key scheme, and the
//! retention selection that decides which backups to prune. No IO — the S3 and
//! Postgres shells call into these so the layout and pruning logic stay testable.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// Current manifest schema version, bumped if the shape changes so an old
/// manifest can still be recognised (and migrated) rather than misread.
pub(crate) const MANIFEST_SCHEMA: u32 = 1;

/// Whether an artifact is an automatic/manual backup of a *live* instance or a
/// durable archive of a torn-down one. Drives the key prefix and how restore
/// treats it.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ArtifactKind {
    Backup,
    Archive,
}

/// The self-describing sidecar written next to every tarball. It carries
/// everything a restore/recover needs to recreate the instance without the
/// Postgres index, so the bucket alone is enough to rebuild the catalog.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub(crate) struct BackupManifest {
    pub(crate) schema: u32,
    pub(crate) kind: ArtifactKind,
    /// Original instance (server) name.
    pub(crate) instance: String,
    pub(crate) game: String,
    /// Owning Discord channel id.
    pub(crate) channel: String,
    /// Discord user id that triggered it, or `auto` for the scheduled cycle.
    pub(crate) created_by: String,
    /// RFC 3339 creation time.
    pub(crate) created_at: String,
    /// S3 key of the compressed tarball this manifest describes.
    pub(crate) tarball_key: String,
    /// Compressed size of the tarball in bytes.
    pub(crate) size_bytes: u64,
}

/// The `auto` sentinel recorded as `created_by` for scheduled backups.
pub(crate) const CREATED_BY_AUTO: &str = "auto";

/// A backup/archive tarball key and its manifest key, kept together so the two
/// are always constructed as a pair.
pub(crate) struct ArtifactKeys {
    pub(crate) tarball: String,
    pub(crate) manifest: String,
}

/// Prefix under which an instance's automatic/manual backups live. The instance
/// is its own index — enumerating this prefix lists its backups, no DB needed.
pub(crate) fn backup_prefix(instance: &str) -> String {
    format!("backups/{instance}/")
}

/// Prefix under which a channel's archives of `name` live.
pub(crate) fn archive_prefix(channel: &str, name: &str) -> String {
    format!("archives/{channel}/{name}/")
}

/// Keys for a backup of `instance` stamped at `stamp` (see [`stamp_now`]).
pub(crate) fn backup_keys(instance: &str, stamp: &str) -> ArtifactKeys {
    keys(&format!("{}{stamp}", backup_prefix(instance)))
}

/// Keys for an archive of `name` in `channel` stamped at `stamp`.
pub(crate) fn archive_keys(channel: &str, name: &str, stamp: &str) -> ArtifactKeys {
    keys(&format!("{}{stamp}", archive_prefix(channel, name)))
}

fn keys(base: &str) -> ArtifactKeys {
    ArtifactKeys {
        tarball: format!("{base}.tar.zst"),
        manifest: format!("{base}.manifest.json"),
    }
}

/// A sortable, filename-safe UTC timestamp segment (`20260707T143000Z`) for the
/// current instant. Lexicographic order matches chronological order, so keys
/// under a prefix list oldest-first without parsing.
pub(crate) fn stamp_now() -> String {
    stamp(Timestamp::now())
}

fn stamp(ts: Timestamp) -> String {
    ts.strftime("%Y%m%dT%H%M%SZ").to_string()
}

/// The tarball object keys under a backup prefix that should be pruned to keep
/// only the `keep` newest. `tarball_keys` need not be pre-sorted; they are ranked
/// lexicographically (== chronologically, given [`stamp_now`]'s format), and
/// everything older than the newest `keep` is returned oldest-first.
pub(crate) fn keys_to_prune(mut tarball_keys: Vec<String>, keep: usize) -> Vec<String> {
    tarball_keys.sort();
    let prune_count = tarball_keys.len().saturating_sub(keep);
    tarball_keys.truncate(prune_count);
    tarball_keys
}

/// The manifest key paired with a tarball key (`…​.tar.zst` → `…​.manifest.json`),
/// so pruning can delete both halves of an artifact from just the tarball key.
pub(crate) fn manifest_key_for(tarball_key: &str) -> Option<String> {
    tarball_key
        .strip_suffix(".tar.zst")
        .map(|base| format!("{base}.manifest.json"))
}

#[cfg(test)]
#[path = "tests/manifest.rs"]
mod tests;
