use super::*;

use std::sync::atomic::{AtomicU32, Ordering};

#[test]
fn a_healthy_process_reports_no_failing_cycles() {
    assert!(CycleHealth::default().failing().is_empty());
}

#[test]
fn one_panic_does_not_fail_the_probe() {
    // Restarting the pod for a single bad tick would cost more than the tick did.
    let health = CycleHealth::default();
    health.record_panic("backup");
    assert!(health.failing().is_empty());
}

#[test]
fn repeated_panics_in_one_cycle_fail_the_probe() {
    let health = CycleHealth::default();
    for _ in 0..UNHEALTHY_PANIC_THRESHOLD {
        health.record_panic("sampler");
    }
    assert_eq!(
        health.failing(),
        vec![("sampler", UNHEALTHY_PANIC_THRESHOLD)]
    );
}

#[test]
fn cycles_are_counted_separately() {
    // One flaky cycle must not drag another over the threshold with it.
    let health = CycleHealth::default();
    for _ in 0..UNHEALTHY_PANIC_THRESHOLD {
        health.record_panic("sampler");
    }
    health.record_panic("backup");
    assert_eq!(
        health.failing(),
        vec![("sampler", UNHEALTHY_PANIC_THRESHOLD)]
    );
}

#[tokio::test]
async fn a_panicking_cycle_is_restarted_and_counted() {
    let health = CycleHealth::new();
    let tasks = TaskTracker::new();
    let shutdown = CancellationToken::new();
    let attempts = Arc::new(AtomicU32::new(0));

    let counter = Arc::clone(&attempts);
    let stop = shutdown.clone();
    spawn_supervised("flaky", &health, &tasks, shutdown.clone(), move || {
        let counter = Arc::clone(&counter);
        let stop = stop.clone();
        async move {
            let attempt = counter.fetch_add(1, Ordering::SeqCst);
            assert!(attempt >= 2, "boom");
            // Third attempt: stay alive until shutdown, like a real cycle.
            stop.cancelled().await;
        }
    });

    // Two panics, each waiting out the restart backoff before the next attempt.
    tokio::time::sleep(RESTART_BACKOFF * 2 + Duration::from_secs(2)).await;
    assert!(
        attempts.load(Ordering::SeqCst) >= 3,
        "the cycle should have been restarted after each panic, got {} attempts",
        attempts.load(Ordering::SeqCst)
    );
    assert_eq!(
        health.failing(),
        Vec::new(),
        "two panics is under the threshold"
    );

    shutdown.cancel();
    tasks.close();
    tasks.wait().await;
}

#[tokio::test]
async fn a_cycle_that_returns_cleanly_is_not_restarted() {
    let health = CycleHealth::new();
    let tasks = TaskTracker::new();
    let shutdown = CancellationToken::new();
    let attempts = Arc::new(AtomicU32::new(0));

    let counter = Arc::clone(&attempts);
    spawn_supervised("tidy", &health, &tasks, shutdown, move || {
        let counter = Arc::clone(&counter);
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });

    tasks.close();
    tasks.wait().await;
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "a clean return means the cycle is done, not that it needs restarting"
    );
}
