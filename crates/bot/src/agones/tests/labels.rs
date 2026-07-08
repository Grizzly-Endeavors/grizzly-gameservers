use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::{ServicePort, ServiceSpec};

use super::*;

fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

/// A `NodePort` Service targeting `gameserver` on `node_port`.
fn service(gameserver: &str, node_port: Option<i32>) -> Service {
    Service {
        spec: Some(ServiceSpec {
            selector: Some(labels(&[(GAMESERVER_SELECTOR_KEY, gameserver)])),
            ports: Some(vec![ServicePort {
                node_port,
                ..Default::default()
            }]),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[test]
fn managed_label_is_recognized() {
    let map = labels(&[(MANAGED_BY_KEY, MANAGED_BY_VALUE)]);
    assert!(is_managed(Some(&map)));
}

#[test]
fn other_managed_by_value_is_not_ours() {
    let map = labels(&[(MANAGED_BY_KEY, "flux")]);
    assert!(
        !is_managed(Some(&map)),
        "the GitOps singleton must be refused"
    );
}

#[test]
fn missing_labels_are_not_managed() {
    assert!(!is_managed(None));
    assert!(!is_managed(Some(&BTreeMap::new())));
}

#[test]
fn label_value_reads_present_and_absent_keys() {
    let map = labels(&[(GAME_KEY, "minecraft")]);
    assert_eq!(label_value(Some(&map), GAME_KEY), Some("minecraft"));
    assert_eq!(label_value(Some(&map), GUILD_KEY), None);
    assert_eq!(label_value(None, GAME_KEY), None);
}

#[test]
fn service_node_port_reads_the_first_port() {
    assert_eq!(
        service_node_port(&service("mc-abc", Some(30001))),
        Some(30001)
    );
    assert_eq!(service_node_port(&service("mc-abc", None)), None);
    assert_eq!(service_node_port(&Service::default()), None);
}

#[test]
fn service_gameserver_target_reads_the_selector() {
    assert_eq!(
        service_gameserver_target(&service("mc-abc", Some(30001))),
        Some("mc-abc")
    );
    assert_eq!(service_gameserver_target(&Service::default()), None);
}

#[test]
fn node_ports_by_gameserver_maps_only_complete_services() {
    let services = [
        service("mc-one", Some(30001)),
        service("mc-two", Some(30002)),
        service("mc-noport", None), // dropped: no node port
        Service::default(),         // dropped: no selector
    ];
    let map = node_ports_by_gameserver(&services);
    assert_eq!(map.get("mc-one"), Some(&30001));
    assert_eq!(map.get("mc-two"), Some(&30002));
    assert_eq!(map.get("mc-noport"), None);
    assert_eq!(map.len(), 2);
}
