use std::collections::BTreeMap;
use std::path::PathBuf;

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

/// The supervisor's own default when `SUPERVISOR_GAME_PORT` is unset — mirrors
/// `DEFAULT_GAME_PORT` in `crates/supervisor/src/config.rs`. Minecraft relies on
/// this default equalling its own game port instead of setting the env explicitly.
const SUPERVISOR_DEFAULT_GAME_PORT: u16 = 25565;

/// Pulls the value out of an uncommented `ENV KEY=value` line. Deliberately a
/// line scan, not a Dockerfile parser — good enough for the flat `ENV` lines
/// every game template uses, and cheap to keep in sync with them.
fn dockerfile_env<'a>(dockerfile: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("ENV {key}=");
    dockerfile
        .lines()
        .map(str::trim_start)
        .filter(|line| !line.starts_with('#'))
        .find_map(|line| line.strip_prefix(prefix.as_str()))
}

/// Collects `(name, containerPort)` pairs out of a `GameServer` manifest's port
/// list entries (top-level `spec.ports` and the container `ports`, which always
/// mirror each other), skipping the pod-internal `control` port — it's never a
/// candidate for the game's own readiness probe.
fn manifest_container_ports(manifest: &str) -> Vec<(String, u16)> {
    let mut ports = Vec::new();
    let mut current_name: Option<String> = None;
    for line in manifest.lines() {
        let trimmed = line.trim_start();
        if let Some(name) = trimmed.strip_prefix("- name: ") {
            current_name = Some(name.trim().to_owned());
        } else if let Some(value) = trimmed.strip_prefix("containerPort: ")
            && let Some(name) = current_name.take()
            && name != "control"
            && let Ok(port) = value.trim().parse()
        {
            ports.push((name, port));
        }
    }
    ports
}

/// Guards the contract between each real game's `SUPERVISOR_GAME_PORT` (or its
/// implicit default) and the port the manifest actually exposes it on, so a typo
/// is caught here rather than only discovered when a fresh server's readiness
/// probe never succeeds.
///
/// `SUPERVISOR_GAME_PORT` either names a declared game `containerPort`
/// (Minecraft's implicit default, Terraria, Satisfactory's template default) or
/// the game's own `SUPERVISOR_RCON_PORT` (Factorio, Palworld: their RCON port is
/// what actually opens once the game is hosting, and it's intentionally
/// pod-internal — never a `containerPort` — see their `Dockerfile`s). A game on
/// the log-marker readiness path (`SUPERVISOR_READY_LOG_PATTERN`, e.g. Valheim)
/// never consults `game_port` for readiness at all (see `runner.rs`'s
/// `spawn_readiness_probe` gate), so it's exempt from the check entirely.
#[test]
fn real_game_ports_agree_with_their_readiness_probe() {
    let games_dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../games"));
    let mut checked = Vec::new();
    for entry in std::fs::read_dir(&games_dir).unwrap() {
        let entry = entry.unwrap();
        if !entry.file_type().unwrap().is_dir() {
            continue;
        }
        let game = entry.file_name().into_string().unwrap();
        if game == "_template" {
            continue;
        }
        let dockerfile = std::fs::read_to_string(entry.path().join("Dockerfile")).unwrap();
        if dockerfile_env(&dockerfile, "SUPERVISOR_READY_LOG_PATTERN").is_some() {
            continue;
        }
        let manifest = std::fs::read_to_string(entry.path().join("gameserver.yaml")).unwrap();

        let game_port = dockerfile_env(&dockerfile, "SUPERVISOR_GAME_PORT").map_or(
            SUPERVISOR_DEFAULT_GAME_PORT,
            |raw| {
                raw.parse::<u16>().unwrap_or_else(|_| {
                    panic!("{game}: SUPERVISOR_GAME_PORT {raw:?} isn't a valid port")
                })
            },
        );
        let rcon_port = dockerfile_env(&dockerfile, "SUPERVISOR_RCON_PORT").map(|raw| {
            raw.parse::<u16>().unwrap_or_else(|_| {
                panic!("{game}: SUPERVISOR_RCON_PORT {raw:?} isn't a valid port")
            })
        });
        let container_ports = manifest_container_ports(&manifest);

        let matches_container_port = container_ports.iter().any(|(_, port)| *port == game_port);
        let matches_rcon_port = rcon_port == Some(game_port);
        assert!(
            matches_container_port || matches_rcon_port,
            "{game}: SUPERVISOR_GAME_PORT {game_port} matches neither a declared \
             containerPort ({container_ports:?}) nor SUPERVISOR_RCON_PORT ({rcon_port:?})"
        );
        checked.push(game);
    }
    assert!(
        !checked.is_empty(),
        "expected to find at least one real game to check under {games_dir:?}"
    );
}
