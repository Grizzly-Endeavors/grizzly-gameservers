use super::*;

use std::ffi::OsStr;

#[tokio::test]
async fn output_under_the_cap_is_kept_whole() {
    let payload = vec![b'x'; 128];
    let (kept, truncated) = read_capped(payload.as_slice()).await;
    assert_eq!(kept, payload);
    assert!(!truncated);
}

#[tokio::test]
async fn output_past_the_cap_is_cut_and_flagged() {
    let payload = vec![b'x'; OUTPUT_CAP_BYTES + 4096];
    let (kept, truncated) = read_capped(payload.as_slice()).await;
    assert_eq!(kept.len(), OUTPUT_CAP_BYTES);
    assert!(truncated);
}

#[tokio::test]
async fn output_exactly_at_the_cap_is_not_flagged() {
    // The boundary matters: reporting truncation when nothing was dropped would
    // send the agent hunting for output that does not exist.
    let payload = vec![b'x'; OUTPUT_CAP_BYTES];
    let (kept, truncated) = read_capped(payload.as_slice()).await;
    assert_eq!(kept.len(), OUTPUT_CAP_BYTES);
    assert!(!truncated);
}

#[tokio::test]
async fn empty_output_reads_clean() {
    let (kept, truncated) = read_capped(b"".as_slice()).await;
    assert!(kept.is_empty());
    assert!(!truncated);
}

#[test]
fn procfs_and_sysfs_are_not_readable_roots() {
    // These two carry the supervisor's own environment, and with it the control
    // token and the game's RCON password.
    assert!(!readable_root(OsStr::new("proc")));
    assert!(!readable_root(OsStr::new("sys")));
}

#[test]
fn ordinary_roots_are_readable() {
    for name in ["usr", "opt", "config", "data", "etc", "tmp"] {
        assert!(readable_root(OsStr::new(name)), "{name} should be readable");
    }
}

#[test]
fn the_sandbox_abi_can_restrict_truncation() {
    // Below v3 a "read-only" shell could still empty a config file in place,
    // which would make the guarantee this module states false.
    assert!(SANDBOX_ABI >= ABI::V3);
}
