//! Palworld config seeding.
//!
//! Palworld's upstream image (thijsvanloef/palworld-server-docker) rewrites the
//! entire `PalWorldSettings.ini` from environment variables on every boot, which
//! would wipe any per-instance edit the friend or the ops agent makes to the file
//! (a blank `SERVER_PASSWORD` env, for instance, resets the join password each
//! restart). The image disables that regeneration (`DISABLE_GENERATE_SETTINGS=true`)
//! so the on-PVC ini is authoritative; in exchange the supervisor takes ownership
//! of just the RCON keys, seeding them into the ini before each launch so RCON
//! still comes up on the minted password. Every other setting — including the
//! join `ServerPassword` — is left exactly as the ini has it.

use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, Result};

/// The ini section Palworld reads its world settings from.
const SECTION_HEADER: &str = "[/Script/Pal.PalGameWorldSettings]";
/// The single key that carries every world setting as a flat tuple.
const OPTION_SETTINGS_PREFIX: &str = "OptionSettings=";

/// Seed the RCON keys into the Palworld ini at `path`, preserving every other
/// setting, creating the file (and its parent dirs) on a fresh PVC. Runs before
/// each launch, so a password rotated on a new pod and a first-ever boot both
/// converge on an ini the game will accept.
///
/// # Errors
///
/// Returns an error if the ini can't be read (for a reason other than "not yet
/// created"), its parent directory can't be created, or the write fails.
pub(crate) fn seed_rcon(path: &Path, port: u16, password: &str) -> Result<()> {
    let existing = match std::fs::read_to_string(path) {
        Ok(contents) => Some(contents),
        Err(err) if err.kind() == ErrorKind::NotFound => None,
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to read palworld ini at {}", path.display()));
        }
    };
    let updated = ensure_rcon_settings(existing.as_deref(), port, password);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("failed to create palworld config dir {}", parent.display())
        })?;
    }
    std::fs::write(path, updated)
        .with_context(|| format!("failed to write palworld ini at {}", path.display()))
}

/// Return the ini text with RCON enabled on `port` and `password` set as the
/// `AdminPassword` (which Palworld reuses as the RCON password), preserving all
/// other keys and lines. When `existing` has no `OptionSettings` line — a fresh
/// PVC — a minimal valid ini is produced instead; Palworld fills every key it
/// doesn't find with its own default.
fn ensure_rcon_settings(existing: Option<&str>, port: u16, password: &str) -> String {
    let port = port.to_string();
    if let Some(text) = existing
        && text.lines().any(is_option_settings)
    {
        let mut rebuilt = String::new();
        for line in text.lines() {
            if is_option_settings(line) {
                rebuilt.push_str(&rebuild_option_settings(line, &port, password));
            } else {
                rebuilt.push_str(line);
            }
            rebuilt.push('\n');
        }
        return rebuilt;
    }
    format!(
        "{SECTION_HEADER}\nOptionSettings=(RCONEnabled=True,RCONPort={port},AdminPassword=\"{password}\")\n"
    )
}

/// Whether `line` is the `OptionSettings=(...)` line (ignoring leading indent).
fn is_option_settings(line: &str) -> bool {
    line.trim_start().starts_with(OPTION_SETTINGS_PREFIX)
}

/// Rebuild one `OptionSettings` line with the RCON keys upserted, preserving the
/// line's leading indent and every other key in its original order.
fn rebuild_option_settings(line: &str, port: &str, password: &str) -> String {
    let trimmed = line.trim_start();
    let indent = line.strip_suffix(trimmed).unwrap_or("");
    let value = trimmed.strip_prefix(OPTION_SETTINGS_PREFIX).unwrap_or("");
    let inside = value
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or(value);

    let mut pairs = split_top_level(inside);
    upsert(&mut pairs, "RCONEnabled", "True".to_owned());
    upsert(&mut pairs, "RCONPort", port.to_owned());
    upsert(&mut pairs, "AdminPassword", format!("\"{password}\""));

    let joined = pairs
        .iter()
        .map(|(key, val)| format!("{key}={val}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("{indent}{OPTION_SETTINGS_PREFIX}({joined})")
}

/// Split the inside of an `OptionSettings` tuple into its top-level `Key=Value`
/// pairs, honoring double-quoted values (which may contain commas) and any nested
/// parens. Empty segments — e.g. from a trailing comma — are dropped.
fn split_top_level(inside: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut depth: i32 = 0;
    let mut in_quotes = false;
    let mut current = String::new();
    for ch in inside.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                current.push(ch);
            }
            '(' if !in_quotes => {
                depth += 1;
                current.push(ch);
            }
            ')' if !in_quotes => {
                depth -= 1;
                current.push(ch);
            }
            ',' if !in_quotes && depth == 0 => {
                push_pair(&mut pairs, &current);
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    push_pair(&mut pairs, &current);
    pairs
}

/// Parse one `Key=Value` segment and append it, dropping blanks. A token with no
/// `=` is kept as a key with an empty value so it round-trips untouched.
fn push_pair(pairs: &mut Vec<(String, String)>, segment: &str) {
    let segment = segment.trim();
    if segment.is_empty() {
        return;
    }
    match segment.split_once('=') {
        Some((key, value)) => pairs.push((key.trim().to_owned(), value.to_owned())),
        None => pairs.push((segment.to_owned(), String::new())),
    }
}

/// Set `key` to `value`, replacing an existing entry in place (preserving its
/// position in the tuple) or appending it when absent.
fn upsert(pairs: &mut Vec<(String, String)>, key: &str, value: String) {
    if let Some(entry) = pairs.iter_mut().find(|(existing, _)| existing == key) {
        entry.1 = value;
    } else {
        pairs.push((key.to_owned(), value));
    }
}

#[cfg(test)]
#[path = "tests/palworld.rs"]
mod tests;
