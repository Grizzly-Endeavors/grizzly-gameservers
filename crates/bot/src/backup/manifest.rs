use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::domain::{GameId, GuildId, InstanceName};

pub(crate) const MANIFEST_SCHEMA: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ArtifactKind {
    Backup,
    Archive,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub(crate) struct BackupManifest {
    pub(crate) schema: u32,
    pub(crate) kind: ArtifactKind,
    pub(crate) instance: InstanceName,
    pub(crate) game: GameId,
    pub(crate) guild: GuildId,
    pub(crate) created_by: String,
    pub(crate) created_at: String,
    pub(crate) tarball_key: String,
    pub(crate) size_bytes: u64,
}

pub(crate) const CREATED_BY_AUTO: &str = "auto";

pub(crate) struct ArtifactKeys {
    pub(crate) tarball: String,
    pub(crate) manifest: String,
}

pub(crate) const UNSCOPED_GUILD_SEGMENT: &str = "_unscoped";

fn guild_segment(guild: &str) -> &str {
    if guild.is_empty() {
        UNSCOPED_GUILD_SEGMENT
    } else {
        guild
    }
}

pub(crate) fn backup_prefix(guild: &str, instance: &str) -> String {
    format!("backups/{}/{instance}/", guild_segment(guild))
}

pub(crate) fn archive_prefix(guild: &str, name: &str) -> String {
    format!("archives/{}/{name}/", guild_segment(guild))
}

pub(crate) fn backup_keys(guild: &str, instance: &str, stamp: &str) -> ArtifactKeys {
    keys(&format!("{}{stamp}", backup_prefix(guild, instance)))
}

pub(crate) fn archive_keys(guild: &str, name: &str, stamp: &str) -> ArtifactKeys {
    keys(&format!("{}{stamp}", archive_prefix(guild, name)))
}

fn keys(base: &str) -> ArtifactKeys {
    ArtifactKeys {
        tarball: format!("{base}.tar.zst"),
        manifest: format!("{base}.manifest.json"),
    }
}

pub(crate) fn stamp_now() -> String {
    stamp(Timestamp::now())
}

fn stamp(ts: Timestamp) -> String {
    ts.strftime("%Y%m%dT%H%M%SZ").to_string()
}

pub(crate) fn keys_to_prune(mut tarball_keys: Vec<String>, keep: usize) -> Vec<String> {
    tarball_keys.sort();
    let prune_count = tarball_keys.len().saturating_sub(keep);
    tarball_keys.truncate(prune_count);
    tarball_keys
}

pub(crate) fn manifest_key_for(tarball_key: &str) -> Option<String> {
    tarball_key
        .strip_suffix(".tar.zst")
        .map(|base| format!("{base}.manifest.json"))
}

#[cfg(test)]
#[path = "tests/manifest.rs"]
mod tests;
