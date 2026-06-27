use super::*;
use k8s_openapi::api::core::v1::PodStatus;

fn pod_with(phase: Option<&str>, ip: Option<&str>) -> Pod {
    Pod {
        status: Some(PodStatus {
            phase: phase.map(str::to_owned),
            pod_ip: ip.map(str::to_owned),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[test]
fn running_pod_with_ip_is_resolved() {
    let pod = pod_with(Some("Running"), Some("10.1.2.3"));
    assert_eq!(
        running_pod_ip(&pod).as_deref(),
        Some("10.1.2.3"),
        "a Running pod with an IP should resolve to that IP"
    );
}

#[test]
fn pending_pod_is_skipped() {
    let pod = pod_with(Some("Pending"), Some("10.1.2.3"));
    assert!(
        running_pod_ip(&pod).is_none(),
        "a not-yet-Running pod should not be used even if it has an IP"
    );
}

#[test]
fn running_pod_without_ip_is_skipped() {
    let pod = pod_with(Some("Running"), None);
    assert!(
        running_pod_ip(&pod).is_none(),
        "a Running pod without an IP yet should not resolve"
    );
}

#[test]
fn result_kinds_map_to_their_outcomes() {
    assert!(matches!(
        map_result_kind(ResultKind::Stopping),
        SupervisorOutcome::Paused
    ));
    assert!(matches!(
        map_result_kind(ResultKind::AlreadyStopped),
        SupervisorOutcome::AlreadyStopped
    ));
    assert!(matches!(
        map_result_kind(ResultKind::Starting),
        SupervisorOutcome::Resumed
    ));
    assert!(matches!(
        map_result_kind(ResultKind::AlreadyRunning),
        SupervisorOutcome::AlreadyRunning
    ));
    assert!(matches!(
        map_result_kind(ResultKind::Restarting),
        SupervisorOutcome::Restarted
    ));
}
