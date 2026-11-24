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

const SECTION_HEADER: &str = "[/Script/Pal.PalGameWorldSettings]";
const OPTION_SETTINGS_PREFIX: &str = "OptionSettings=";

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

fn is_option_settings(line: &str) -> bool {
    line.trim_start().starts_with(OPTION_SETTINGS_PREFIX)
}

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
