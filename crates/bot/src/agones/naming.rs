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

/// Length of the random id appended to auto-named instances.
const GENERATED_ID_LEN: usize = 5;

const BASE36: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Derive the PVC name for an instance. Kept here so the `<instance>-data`
/// convention has a single definition shared by the renderer and the apply layer.
pub(crate) fn pvc_name(instance: &str) -> String {
    format!("{instance}{PVC_SUFFIX}")
}

/// Build the instance name for a `/create`. When `raw` is `Some`, the
/// friend-supplied world name is used verbatim (sanitized to an RFC1123 label
/// segment) — no game prefix, since the game is tracked as a label and read
/// back from there. When `None`, a `<game>-<id>` name is generated from
/// `entropy` (injected so the function stays pure and testable) so unnamed
/// worlds are still identifiable at a glance.
///
/// # Errors
///
/// Returns an error if a supplied name has no usable alphanumeric characters,
/// or if the resulting name would exceed the length budget.
pub(crate) fn build_instance_name(game: &str, raw: Option<&str>, entropy: u64) -> Result<String> {
    let name = match raw {
        Some(supplied) => sanitize_label_segment(supplied)?,
        None => format!("{game}-{}", base36_suffix(entropy, GENERATED_ID_LEN)),
    };
    if name.len() > MAX_INSTANCE_LEN {
        bail!("name '{name}' is too long (max {MAX_INSTANCE_LEN} characters)");
    }
    Ok(name)
}

/// Coerce arbitrary text into a lowercase RFC1123 label segment: alphanumerics
/// pass through, runs of other characters collapse to single dashes, and
/// leading/trailing dashes are trimmed. Rejects (rather than silently mangles)
/// input that contains no alphanumeric characters.
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

/// Encode `value` as a fixed-length lowercase base36 string. Collisions are
/// possible but checked at creation time (the API rejects a duplicate name).
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

/// Clock-derived entropy for generated instance ids. Not security-sensitive —
/// uniqueness is ultimately enforced by the API rejecting a duplicate name.
pub(crate) fn now_entropy() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
        })
}

/// Pick the lowest port in `range` that is neither in `used` nor `excluded`.
/// Returns `None` when every port in the range is taken.
pub(crate) fn select_free_port(
    used: &BTreeSet<i32>,
    excluded: &BTreeSet<i32>,
    range: RangeInclusive<i32>,
) -> Option<i32> {
    range
        .into_iter()
        .find(|port| !used.contains(port) && !excluded.contains(port))
}

#[cfg(test)]
#[path = "tests/naming.rs"]
mod tests;
