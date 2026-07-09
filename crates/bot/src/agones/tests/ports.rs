use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::{ServicePort, ServiceSpec};

use super::super::labels::GAMESERVER_SELECTOR_KEY;
use super::*;

fn annotations(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

/// The four annotations a two-port advertised game (Satisfactory-shaped) carries.
fn advertised_annotations() -> BTreeMap<String, String> {
    annotations(&[
        (ADVERTISED_PORTS_ANNOTATION, "game,messaging"),
        (FRIEND_FACING_ANNOTATION, "game"),
        (
            "grizzly-gameservers.grizzly-endeavors.com/port-env.game",
            "SERVERGAMEPORT,SUPERVISOR_GAME_PORT",
        ),
        (
            "grizzly-gameservers.grizzly-endeavors.com/port-env.messaging",
            "SERVERMESSAGINGPORT",
        ),
    ])
}

fn advertised_service(ports: &[(&str, Option<i32>)]) -> Service {
    Service {
        metadata: kube::core::ObjectMeta {
            annotations: Some(advertised_annotations()),
            ..Default::default()
        },
        spec: Some(ServiceSpec {
            selector: Some(
                [(GAMESERVER_SELECTOR_KEY.to_owned(), "sf-abc".to_owned())]
                    .into_iter()
                    .collect(),
            ),
            ports: Some(
                ports
                    .iter()
                    .map(|(name, node_port)| ServicePort {
                        name: Some((*name).to_owned()),
                        node_port: *node_port,
                        ..Default::default()
                    })
                    .collect(),
            ),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[test]
fn no_annotations_is_the_remap_path() {
    assert_eq!(parse_port_plan(None).unwrap(), PortPlan::Remap);
    assert_eq!(
        parse_port_plan(Some(&BTreeMap::new())).unwrap(),
        PortPlan::Remap
    );
    assert_eq!(ports_needed(&PortPlan::Remap), 1);
}

#[test]
fn advertised_plan_parses_ports_env_and_friend_facing() {
    let plan = parse_port_plan(Some(&advertised_annotations())).unwrap();
    let PortPlan::Advertised(ports) = &plan else {
        panic!("expected an advertised plan");
    };
    let [game, messaging] = ports.as_slice() else {
        panic!("expected exactly two ports");
    };
    assert_eq!(game.name, "game");
    assert_eq!(game.env, vec!["SERVERGAMEPORT", "SUPERVISOR_GAME_PORT"]);
    assert!(game.friend_facing);
    assert_eq!(messaging.name, "messaging");
    assert_eq!(messaging.env, vec!["SERVERMESSAGINGPORT"]);
    assert!(!messaging.friend_facing);
    assert_eq!(ports_needed(&plan), 2);
}

#[test]
fn missing_friend_facing_annotation_is_rejected() {
    let mut anns = advertised_annotations();
    anns.remove(FRIEND_FACING_ANNOTATION);
    let err = parse_port_plan(Some(&anns)).unwrap_err();
    assert!(err.to_string().contains("friend-facing-port"));
}

#[test]
fn friend_facing_not_in_list_is_rejected() {
    let mut anns = advertised_annotations();
    anns.insert(FRIEND_FACING_ANNOTATION.to_owned(), "control".to_owned());
    let err = parse_port_plan(Some(&anns)).unwrap_err();
    assert!(err.to_string().contains("not in"));
}

#[test]
fn advertised_port_without_env_mapping_is_rejected() {
    let mut anns = advertised_annotations();
    anns.remove("grizzly-gameservers.grizzly-endeavors.com/port-env.messaging");
    let err = parse_port_plan(Some(&anns)).unwrap_err();
    assert!(err.to_string().contains("env mapping"));
}

#[test]
fn empty_advertised_list_is_rejected() {
    let anns = annotations(&[(ADVERTISED_PORTS_ANNOTATION, "  ,  ")]);
    let err = parse_port_plan(Some(&anns)).unwrap_err();
    assert!(err.to_string().contains("no ports"));
}

#[test]
fn plan_from_service_rejects_a_port_with_no_matching_service_entry() {
    // Annotation names `messaging` but the service only declares `game`.
    let svc = advertised_service(&[("game", Some(7003))]);
    let err = port_plan_from_service(&svc).unwrap_err();
    assert!(err.to_string().contains("no matching port"));
}

#[test]
fn assign_binds_leased_numbers_in_order() {
    let plan = parse_port_plan(Some(&advertised_annotations())).unwrap();
    let assignment = assign(plan, &[7003, 7004]).unwrap();
    let PortAssignment::Advertised(ports) = &assignment else {
        panic!("expected advertised assignment");
    };
    let [game, messaging] = ports.as_slice() else {
        panic!("expected exactly two ports");
    };
    assert_eq!(game.name, "game");
    assert_eq!(game.number, 7003);
    assert_eq!(messaging.name, "messaging");
    assert_eq!(messaging.number, 7004);
    assert_eq!(assignment.friend_facing_port(), 7003);
}

#[test]
fn assign_remap_takes_a_single_port() {
    let assignment = assign(PortPlan::Remap, &[7005]).unwrap();
    assert_eq!(assignment, PortAssignment::Remap(7005));
    assert_eq!(assignment.friend_facing_port(), 7005);
}

#[test]
fn assign_rejects_a_port_count_mismatch() {
    let plan = parse_port_plan(Some(&advertised_annotations())).unwrap();
    let err = assign(plan, &[7003]).unwrap_err();
    assert!(err.to_string().contains("needs 2 leased ports"));
    assert!(assign(PortPlan::Remap, &[7003, 7004]).is_err());
}

#[test]
fn assignment_from_service_round_trips_leased_ports() {
    let svc = advertised_service(&[("game", Some(7003)), ("messaging", Some(7004))]);
    let assignment = assignment_from_service(&svc).unwrap();
    let PortAssignment::Advertised(ports) = &assignment else {
        panic!("expected advertised assignment");
    };
    let [game, messaging] = ports.as_slice() else {
        panic!("expected exactly two ports");
    };
    assert_eq!(game.number, 7003);
    assert_eq!(messaging.number, 7004);
    assert_eq!(assignment.friend_facing_port(), 7003);
}

#[test]
fn friend_facing_node_port_picks_the_game_port_not_the_first() {
    // Ports declared messaging-first to prove selection is by friend-facing name,
    // not port order.
    let svc = advertised_service(&[("messaging", Some(7004)), ("game", Some(7003))]);
    assert_eq!(friend_facing_node_port(&svc), Some(7003));
}

#[test]
fn friend_facing_node_port_falls_back_to_first_for_remap() {
    let svc = Service {
        spec: Some(ServiceSpec {
            ports: Some(vec![ServicePort {
                node_port: Some(7000),
                ..Default::default()
            }]),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(friend_facing_node_port(&svc), Some(7000));
}

#[test]
fn node_ports_by_gameserver_uses_the_friend_facing_port() {
    let svc = advertised_service(&[("messaging", Some(7004)), ("game", Some(7003))]);
    let map = node_ports_by_gameserver(&[svc]);
    assert_eq!(map.get("sf-abc"), Some(&7003));
}

/// Guards the contract between the real `games/satisfactory/service.yaml`
/// annotations and this parser, so a manifest typo (or a renamed port) is caught
/// here rather than only at `/create` time.
#[test]
fn real_satisfactory_manifest_is_on_the_advertise_path() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../games/satisfactory/service.yaml"
    );
    let yaml = std::fs::read_to_string(path).unwrap();
    let svc: Service = serde_yaml_ng::from_str(&yaml).unwrap();
    let PortPlan::Advertised(ports) = port_plan_from_service(&svc).unwrap() else {
        panic!("satisfactory should be on the advertise path");
    };
    let names: Vec<&str> = ports.iter().map(|port| port.name.as_str()).collect();
    assert_eq!(names, ["game", "messaging"]);
    let game = ports.iter().find(|port| port.name == "game").unwrap();
    assert!(game.friend_facing, "game is the friend-facing port");
    assert!(game.env.contains(&"SERVERGAMEPORT".to_owned()));
    assert!(game.env.contains(&"SUPERVISOR_GAME_PORT".to_owned()));
}
