use super::*;

fn capture_of(lines: usize) -> Capture {
    let mut capture = Capture::default();
    for index in 0..lines {
        capture.push(format!("line {index}"));
    }
    capture
}

#[test]
fn an_ordinary_boot_is_kept_whole() {
    // The case that actually matters: a full Valheim boot is ~3700 lines, and a
    // crash on startup has to arrive complete rather than sampled.
    let capture = capture_of(3_700);
    let rendered = capture.render();
    assert_eq!(capture.elided, 0);
    assert!(!rendered.contains("elided"));
    assert!(rendered.contains("line 0"));
    assert!(rendered.contains("line 3699"));
}

#[test]
fn both_ends_survive_a_launch_too_long_to_keep() {
    // The two crash shapes sit at opposite ends: a mod that fails to load throws
    // during startup, a server that dies after a week says why in its last lines.
    let capture = capture_of(HEAD_LINES + TAIL_LINES + 5_000);
    let rendered = capture.render();
    assert!(rendered.contains("line 0"), "the start must survive");
    assert!(
        rendered.contains(&format!("line {}", HEAD_LINES + TAIL_LINES + 4_999)),
        "the end must survive"
    );
    assert_eq!(capture.elided, 5_000);
}

#[test]
fn an_elided_middle_is_stated_not_silent() {
    let capture = capture_of(HEAD_LINES + TAIL_LINES + 42);
    assert!(capture.render().contains("42 lines elided"));
}

#[test]
fn memory_is_bounded_however_long_the_launch_runs() {
    let capture = capture_of(HEAD_LINES + TAIL_LINES + 100_000);
    assert_eq!(capture.head.len(), HEAD_LINES);
    assert_eq!(capture.tail.len(), TAIL_LINES);
}

#[test]
fn beginning_a_launch_discards_the_previous_one() {
    // Without this the capture would stack failed boots the way the shared live
    // ring does, which is the defect this module exists to avoid.
    let log = LaunchLog::new();
    log.push("from the old launch".to_owned());
    log.begin();
    log.push("from the new launch".to_owned());
    let rendered = log.render().expect("the new launch produced output");
    assert!(!rendered.contains("old launch"));
    assert!(rendered.contains("new launch"));
}

#[test]
fn a_silent_launch_preserves_nothing() {
    assert!(LaunchLog::new().render().is_none());
}

#[tokio::test]
async fn settle_returns_immediately_when_no_pump_is_open() {
    assert!(LaunchLog::new().settle().await);
}

#[tokio::test]
async fn settle_waits_for_an_open_pump_and_times_out() {
    let log = LaunchLog::new();
    log.pump_started();
    assert!(!log.settle().await, "an unfinished pump must time out");
}

#[tokio::test]
async fn settle_completes_once_the_pump_reports_in() {
    let log = std::sync::Arc::new(LaunchLog::new());
    log.pump_started();
    let finisher = std::sync::Arc::clone(&log);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        finisher.pump_finished();
    });
    assert!(log.settle().await);
}

#[test]
fn preserve_writes_under_the_reserved_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = preserve(dir.path(), "boom\n").unwrap();
    assert!(path.starts_with(dir.path().join(ARTIFACT_DIR)));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "boom\n");
}

#[test]
fn a_pruned_name_is_never_handed_out_again() {
    // Pruning frees the names it deletes. Reusing one would mean the newest
    // artifact carried the name of an already-deleted older one, and a crash
    // loop inside one second would write nothing but that same name.
    let dir = tempfile::tempdir().unwrap();
    let mut seen = std::collections::HashSet::new();
    for index in 0..(KEEP_ARTIFACTS * 3) {
        let path = preserve(dir.path(), &format!("crash {index}\n")).unwrap();
        assert!(seen.insert(path.clone()), "reused {}", path.display());
    }
}

#[test]
fn two_crashes_in_one_second_do_not_overwrite_each_other() {
    // A crash loop produces exactly this, and losing the first attempt's output
    // to the second attempt's filename would defeat the point.
    let dir = tempfile::tempdir().unwrap();
    let first = preserve(dir.path(), "first\n").unwrap();
    let second = preserve(dir.path(), "second\n").unwrap();
    assert_ne!(first, second);
    assert_eq!(std::fs::read_to_string(&first).unwrap(), "first\n");
    assert_eq!(std::fs::read_to_string(&second).unwrap(), "second\n");
}

#[test]
fn preserving_prunes_the_oldest_and_keeps_the_newest() {
    let dir = tempfile::tempdir().unwrap();
    let mut written = Vec::new();
    for index in 0..(KEEP_ARTIFACTS + 3) {
        written.push(preserve(dir.path(), &format!("crash {index}\n")).unwrap());
    }
    let kept: Vec<_> = std::fs::read_dir(dir.path().join(ARTIFACT_DIR))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(kept.len(), KEEP_ARTIFACTS);
    let newest = written.last().unwrap();
    assert!(
        kept.contains(newest),
        "the newest crash must survive pruning; kept {kept:?}"
    );
}
