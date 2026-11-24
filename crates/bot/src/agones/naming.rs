use std::collections::BTreeSet;
use std::ops::RangeInclusive;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};

/// Kubernetes object names are RFC1123 labels capped at 63 chars. The PVC name
/// is derived as `<instance>-data`, so the instance name itself must leave room
/// for that suffix.
const MAX_OBJECT_NAME_LEN: usize = 63;
const PVC_SUFFIX: &str = "-data";
const MAX_INSTANCE_LEN: usize = MAX_OBJECT_NAME_LEN - PVC_SUFFIX.len();

const GENERATED_ID_LEN: usize = 5;

const BASE36: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

pub(crate) fn pvc_name(instance: &str) -> String {
    format!("{instance}{PVC_SUFFIX}")
}

pub(crate) fn build_instance_name(game: &str, raw: Option<&str>, entropy: u64) -> Result<String> {
    if let Some(supplied) = raw {
        return validate_world_name(supplied);
    }
    let name = format!("{game}-{}", base36_suffix(entropy, GENERATED_ID_LEN));
    check_instance_len(&name)?;
    Ok(name)
}

pub(crate) fn validate_world_name(raw: &str) -> Result<String> {
    let name = sanitize_label_segment(raw)?;
    check_instance_len(&name)?;
    Ok(name)
}

fn check_instance_len(name: &str) -> Result<()> {
    if name.len() > MAX_INSTANCE_LEN {
        bail!("name '{name}' is too long (max {MAX_INSTANCE_LEN} characters)");
    }
    Ok(())
}

fn sanitize_label_segment(raw: &str) -> Result<String> {
    let mut out = String::with_capacity(raw.len());
    let mut pending_dash = false;
    for ch in raw.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(lower);
        } else {
            pending_dash = true;
        }
    }
    if out.is_empty() {
        bail!("name must contain at least one letter or number");
    }
    Ok(out)
}

fn base36_suffix(mut value: u64, len: usize) -> String {
    let mut out = String::with_capacity(len);
    for _ in 0..len {
        let index = usize::try_from(value % 36).unwrap_or(0);
        if let Some(&byte) = BASE36.get(index) {
            out.push(char::from(byte));
        }
        value /= 36;
    }
    out
}

pub(crate) fn now_entropy() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
        })
}

pub(crate) fn select_free_ports(
    n: usize,
    used: &BTreeSet<i32>,
    excluded: &BTreeSet<i32>,
    range: RangeInclusive<i32>,
) -> Option<Vec<i32>> {
    let free: Vec<i32> = range
        .into_iter()
        .filter(|port| !used.contains(port) && !excluded.contains(port))
        .take(n)
        .collect();
    (free.len() == n).then_some(free)
}

#[cfg(test)]
#[path = "tests/naming.rs"]
mod tests;
