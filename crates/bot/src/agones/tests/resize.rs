use k8s_openapi::api::core::v1::{
    ContainerStatus, Pod, PodCondition, PodStatus, ResourceRequirements,
};
use std::collections::BTreeMap;

use super::*;

fn sizing() -> Sizing {
    Sizing {
        cpu_request_millicores: 500,
        mem_request_bytes: 2 * 1024 * 1024 * 1024,
        cpu_limit_millicores: 2000,
        mem_limit_bytes: 4 * 1024 * 1024 * 1024,
    }
}

fn quantities(cpu: &str, memory: &str) -> BTreeMap<String, Quantity> {
    BTreeMap::from([
        ("cpu".to_owned(), Quantity(cpu.to_owned())),
        ("memory".to_owned(), Quantity(memory.to_owned())),
    ])
}

/// A pod whose game container reports the given running resources.
fn pod_running(requests: (&str, &str), limits: (&str, &str)) -> Pod {
    Pod {
        status: Some(PodStatus {
            container_statuses: Some(vec![ContainerStatus {
                name: "minecraft".to_owned(),
                resources: Some(ResourceRequirements {
                    requests: Some(quantities(requests.0, requests.1)),
                    limits: Some(quantities(limits.0, limits.1)),
                    ..ResourceRequirements::default()
                }),
                ..ContainerStatus::default()
            }]),
            ..PodStatus::default()
        }),
        ..Pod::default()
    }
}

/// Metadata built through serde rather than by hand, the way it reaches the bot
/// from the API server — timestamp parsing included.
fn meta_from_json(value: serde_json::Value) -> kube::core::ObjectMeta {
    serde_json::from_value(value).expect("metadata fixture should deserialize")
}

fn with_condition(mut pod: Pod, type_: &str, reason: Option<&str>, message: Option<&str>) -> Pod {
    let condition = PodCondition {
        type_: type_.to_owned(),
        status: "True".to_owned(),
        reason: reason.map(str::to_owned),
        message: message.map(str::to_owned),
        ..PodCondition::default()
    };
    if let Some(status) = pod.status.as_mut() {
        status
            .conditions
            .get_or_insert_with(Vec::new)
            .push(condition);
    }
    pod
}

#[test]
fn a_container_reporting_the_new_sizing_counts_as_applied() {
    let pod = pod_running(("500m", "2147483648"), ("2000m", "4294967296"));
    assert!(applied(&pod, "minecraft", sizing()));
}

#[test]
fn normalized_quantities_still_match_what_was_asked_for() {
    // Kubernetes rewrites "2000m" as "2" and byte counts into binary suffixes.
    // Comparing the strings we sent would report a finished resize as forever
    // pending, so the check parses both sides.
    let pod = pod_running(("0.5", "2Gi"), ("2", "4Gi"));
    assert!(applied(&pod, "minecraft", sizing()));
}

#[test]
fn a_container_still_on_its_old_sizing_is_not_applied() {
    let pod = pod_running(("250m", "1073741824"), ("1000m", "2147483648"));
    assert!(!applied(&pod, "minecraft", sizing()));
}

#[test]
fn a_half_applied_resize_is_not_applied() {
    // Requests updated, limits not: reporting this as done would tell a friend
    // their server has headroom it doesn't have.
    let pod = pod_running(("500m", "2147483648"), ("1000m", "2147483648"));
    assert!(!applied(&pod, "minecraft", sizing()));
}

#[test]
fn the_sidecars_resources_are_not_mistaken_for_the_games() {
    // The pod carries the Agones sidecar too; matching by name is what keeps a
    // resize check from reading the wrong container.
    let pod = pod_running(("500m", "2147483648"), ("2000m", "4294967296"));
    assert!(!applied(&pod, "agones-gameserver-sidecar", sizing()));
}

#[test]
fn a_resize_still_in_progress_is_not_reported_as_applied() {
    // The kubelet publishes the new figures before the cgroup write lands, so
    // the in-progress condition is what stops an early "done".
    let pod = with_condition(
        pod_running(("500m", "2147483648"), ("2000m", "4294967296")),
        RESIZE_IN_PROGRESS_CONDITION,
        None,
        None,
    );
    assert!(!applied(&pod, "minecraft", sizing()));
}

#[test]
fn an_infeasible_resize_surfaces_the_kubelets_own_message() {
    let pod = with_condition(
        pod_running(("250m", "1073741824"), ("1000m", "2147483648")),
        RESIZE_PENDING_CONDITION,
        Some(INFEASIBLE_REASON),
        Some("memory limit below current usage"),
    );
    assert_eq!(
        infeasible_message(&pod).as_deref(),
        Some("memory limit below current usage"),
        "the operator needs the real obstacle, not a generic refusal"
    );
}

#[test]
fn a_deferred_resize_is_not_treated_as_infeasible() {
    // Deferred means "the node is full right now" — the resize stays queued, so
    // calling it infeasible would give up on something still likely to happen.
    let pod = with_condition(
        pod_running(("250m", "1073741824"), ("1000m", "2147483648")),
        RESIZE_PENDING_CONDITION,
        Some("Deferred"),
        Some("node has insufficient memory right now"),
    );
    assert_eq!(infeasible_message(&pod), None);
}

#[test]
fn a_pod_with_no_resize_conditions_is_neither_pending_nor_in_progress() {
    let pod = pod_running(("500m", "2147483648"), ("2000m", "4294967296"));
    assert_eq!(infeasible_message(&pod), None);
    assert!(!in_progress(&pod));
}

#[test]
fn a_terminating_pod_is_never_the_resize_target() {
    // A pod on a dead node reports Running indefinitely; resizing it would
    // silently do nothing.
    let mut pod = pod_running(("500m", "2147483648"), ("2000m", "4294967296"));
    pod.metadata = meta_from_json(serde_json::json!({ "name": "mc-abc12" }));
    pod.status.as_mut().expect("status was just built").phase = Some("Running".to_owned());
    assert_eq!(running_name(pod.clone()), Some("mc-abc12".to_owned()));

    pod.metadata = meta_from_json(serde_json::json!({
        "name": "mc-abc12",
        "deletionTimestamp": "2026-07-24T20:45:00Z",
    }));
    assert_eq!(running_name(pod), None);
}

#[test]
fn a_pod_that_is_not_running_is_not_the_resize_target() {
    let mut pod = pod_running(("500m", "2147483648"), ("2000m", "4294967296"));
    pod.metadata.name = Some("mc-abc12".to_owned());
    pod.status.as_mut().expect("status was just built").phase = Some("Pending".to_owned());
    assert_eq!(running_name(pod), None);
}

#[test]
fn a_resolved_condition_left_behind_as_false_does_not_block_a_resize() {
    // The kubelet may clear a condition by flipping it to False rather than
    // removing it. Reading mere presence would strand every later resize on this
    // pod as "still settling" or "infeasible" forever.
    let mut pod = with_condition(
        pod_running(("500m", "2147483648"), ("2000m", "4294967296")),
        RESIZE_IN_PROGRESS_CONDITION,
        None,
        None,
    );
    pod = with_condition(
        pod,
        RESIZE_PENDING_CONDITION,
        Some(INFEASIBLE_REASON),
        Some("stale"),
    );
    for condition in pod
        .status
        .as_mut()
        .expect("status was just built")
        .conditions
        .as_mut()
        .expect("conditions were just added")
    {
        condition.status = "False".to_owned();
    }

    assert!(!in_progress(&pod));
    assert_eq!(infeasible_message(&pod), None);
    assert!(applied(&pod, "minecraft", sizing()));
}

#[test]
fn the_refusal_status_is_the_one_kubernetes_uses_for_a_rejected_resize() {
    // Pinned because the whole 422 branch hangs on it: a wrong value here would
    // send a plain refusal down the transport-error path, where it surfaces to a
    // friend as "I couldn't reach the cluster" — a lie about a cluster that
    // answered clearly.
    assert_eq!(REFUSED_STATUS, 422);
}
