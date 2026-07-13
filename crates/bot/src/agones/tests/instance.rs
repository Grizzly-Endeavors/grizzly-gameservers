use super::*;

const GAMESERVER_TEMPLATE: &str = "
apiVersion: agones.dev/v1
kind: GameServer
metadata:
  name: minecraft
  namespace: game-servers
spec:
  container: minecraft
  template:
    spec:
      containers:
        - name: minecraft
      volumes:
        - name: world
          persistentVolumeClaim:
            claimName: minecraft-data
";

const SERVICE_TEMPLATE: &str = "
apiVersion: v1
kind: Service
metadata:
  name: minecraft
  namespace: game-servers
spec:
  type: NodePort
  selector:
    agones.dev/gameserver: minecraft
  ports:
    - name: minecraft
      port: 25565
      nodePort: 7000
";

const PVC_TEMPLATE: &str = "
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: minecraft-data
  namespace: game-servers
spec:
  storageClassName: iscsi-zfs
  resources:
    requests:
      storage: 5Gi
";

fn entry() -> GameCatalogEntry {
    GameCatalogEntry {
        id: "minecraft".to_owned(),
        gameserver_yaml: GAMESERVER_TEMPLATE.to_owned(),
        service_yaml: SERVICE_TEMPLATE.to_owned(),
        pvc_yaml: PVC_TEMPLATE.to_owned(),
    }
}

fn identity() -> InstanceIdentity {
    InstanceIdentity {
        name: InstanceName::new("minecraft-ab12"),
        game: GameId::new("minecraft"),
        namespace: "game-servers".to_owned(),
        ports: PortAssignment::Remap(7003),
        guild: GuildId::new("555"),
        start_paused: false,
    }
}

const ADVERTISED_GAMESERVER_TEMPLATE: &str = "
apiVersion: agones.dev/v1
kind: GameServer
metadata:
  name: satisfactory
  namespace: game-servers
spec:
  container: satisfactory
  ports:
    - name: game
      portPolicy: None
      containerPort: 7777
      protocol: UDP
    - name: messaging
      portPolicy: None
      containerPort: 8888
      protocol: TCP
  template:
    spec:
      containers:
        - name: satisfactory
          ports:
            - name: game
              containerPort: 7777
              protocol: UDP
            - name: messaging
              containerPort: 8888
              protocol: TCP
            - name: control
              containerPort: 9359
              protocol: TCP
      volumes:
        - name: data
          persistentVolumeClaim:
            claimName: satisfactory-data
";

const ADVERTISED_SERVICE_TEMPLATE: &str = "
apiVersion: v1
kind: Service
metadata:
  name: satisfactory
  namespace: game-servers
  annotations:
    grizzly-gameservers.grizzly-endeavors.com/advertised-ports: game,messaging
    grizzly-gameservers.grizzly-endeavors.com/friend-facing-port: game
    grizzly-gameservers.grizzly-endeavors.com/port-env.game: SERVERGAMEPORT,SUPERVISOR_GAME_PORT
    grizzly-gameservers.grizzly-endeavors.com/port-env.messaging: SERVERMESSAGINGPORT
spec:
  type: NodePort
  selector:
    agones.dev/gameserver: satisfactory
  ports:
    - name: game
      port: 7777
      targetPort: 7777
      nodePort: 7777
      protocol: UDP
    - name: messaging
      port: 8888
      targetPort: 8888
      nodePort: 8888
      protocol: TCP
";

fn advertised_entry() -> GameCatalogEntry {
    GameCatalogEntry {
        id: "satisfactory".to_owned(),
        gameserver_yaml: ADVERTISED_GAMESERVER_TEMPLATE.to_owned(),
        service_yaml: ADVERTISED_SERVICE_TEMPLATE.to_owned(),
        pvc_yaml: PVC_TEMPLATE.to_owned(),
    }
}

fn advertised_identity() -> InstanceIdentity {
    InstanceIdentity {
        name: InstanceName::new("satisfactory-xy99"),
        game: GameId::new("satisfactory"),
        namespace: "game-servers".to_owned(),
        ports: PortAssignment::Advertised(vec![
            AssignedPort {
                name: "game".to_owned(),
                number: 7003,
                env: vec![
                    "SERVERGAMEPORT".to_owned(),
                    "SUPERVISOR_GAME_PORT".to_owned(),
                ],
                friend_facing: true,
            },
            AssignedPort {
                name: "messaging".to_owned(),
                number: 7004,
                env: vec!["SERVERMESSAGINGPORT".to_owned()],
                friend_facing: false,
            },
        ]),
        guild: GuildId::new("555"),
        start_paused: false,
    }
}

#[test]
fn gameserver_gets_instance_identity_and_rebound_claim() {
    let gs = render_gameserver(&entry(), &identity()).unwrap();

    assert_eq!(gs.metadata.name.as_deref(), Some("minecraft-ab12"));
    assert_eq!(gs.metadata.namespace.as_deref(), Some("game-servers"));

    let labels = gs.metadata.labels.as_ref().unwrap();
    assert_eq!(labels.get(MANAGED_BY_KEY).unwrap(), MANAGED_BY_VALUE);
    assert_eq!(labels.get(GAME_KEY).unwrap(), "minecraft");
    assert_eq!(labels.get(INSTANCE_KEY).unwrap(), "minecraft-ab12");
    assert_eq!(labels.get(GUILD_KEY).unwrap(), "555");

    let claim = gs
        .data
        .pointer("/spec/template/spec/volumes/0/persistentVolumeClaim/claimName")
        .and_then(Value::as_str)
        .unwrap();
    assert_eq!(claim, "minecraft-ab12-data");
}

#[test]
fn gameserver_leaves_unrelated_spec_fields_untouched() {
    let gs = render_gameserver(&entry(), &identity()).unwrap();
    assert_eq!(
        gs.data.pointer("/spec/container").and_then(Value::as_str),
        Some("minecraft"),
        "the owning-container field must survive rendering"
    );
}

#[test]
fn empty_guild_leaves_the_guild_label_off() {
    // A pre-scoping instance cold-started from a Service with no guild label
    // must not be stamped with an empty "" guild — the label is simply absent.
    let mut unscoped = identity();
    unscoped.guild = GuildId::new("");
    let gs = render_gameserver(&entry(), &unscoped).unwrap();
    let labels = gs.metadata.labels.as_ref().unwrap();
    assert!(!labels.contains_key(GUILD_KEY));
}

#[test]
fn start_paused_injects_the_supervisor_env_on_the_container() {
    let mut paused = identity();
    paused.start_paused = true;
    let gs = render_gameserver(&entry(), &paused).unwrap();
    let env = gs
        .data
        .pointer("/spec/template/spec/containers/0/env")
        .and_then(Value::as_array)
        .expect("container should have an env list after injection");
    let paused_env = env
        .iter()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some("SUPERVISOR_START_PAUSED"))
        .expect("SUPERVISOR_START_PAUSED should be present");
    assert_eq!(
        paused_env.get("value").and_then(Value::as_str),
        Some("true")
    );
}

#[test]
fn unpaused_render_adds_no_start_paused_env() {
    let gs = render_gameserver(&entry(), &identity()).unwrap();
    let has_pause_env = gs
        .data
        .pointer("/spec/template/spec/containers/0/env")
        .and_then(Value::as_array)
        .is_some_and(|env| {
            env.iter().any(|entry| {
                entry.get("name").and_then(Value::as_str) == Some("SUPERVISOR_START_PAUSED")
            })
        });
    assert!(!has_pause_env, "a normal server must not be paused");
}

#[test]
fn service_selects_instance_and_takes_leased_port() {
    let svc = render_service(&entry(), &identity()).unwrap();
    assert_eq!(svc.metadata.name.as_deref(), Some("minecraft-ab12"));

    let spec = svc.spec.as_ref().unwrap();
    let selector = spec.selector.as_ref().unwrap();
    assert_eq!(
        selector.get(GAMESERVER_SELECTOR_KEY).unwrap(),
        "minecraft-ab12"
    );
    let port = spec.ports.as_ref().unwrap().first().unwrap();
    assert_eq!(port.node_port, Some(7003));
}

#[test]
fn advertised_service_sets_nodeport_equal_to_targetport_per_named_port() {
    let svc = render_service(&advertised_entry(), &advertised_identity()).unwrap();
    let ports = svc.spec.as_ref().unwrap().ports.as_ref().unwrap();

    let game = ports
        .iter()
        .find(|p| p.name.as_deref() == Some("game"))
        .unwrap();
    assert_eq!(game.node_port, Some(7003));
    assert_eq!(game.port, 7003);
    assert_eq!(
        game.target_port,
        Some(k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(7003))
    );

    let messaging = ports
        .iter()
        .find(|p| p.name.as_deref() == Some("messaging"))
        .unwrap();
    assert_eq!(messaging.node_port, Some(7004));
    assert_eq!(messaging.port, 7004);
    assert_eq!(
        messaging.target_port,
        Some(k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(7004))
    );
}

#[test]
fn advertised_gameserver_rewrites_container_ports_and_injects_env() {
    let gs = render_gameserver(&advertised_entry(), &advertised_identity()).unwrap();

    // Agones spec.ports rewritten by name.
    let agones_ports = gs
        .data
        .pointer("/spec/ports")
        .and_then(Value::as_array)
        .unwrap();
    let agones_game = agones_ports
        .iter()
        .find(|p| p.get("name").and_then(Value::as_str) == Some("game"))
        .unwrap();
    assert_eq!(
        agones_game.get("containerPort").and_then(Value::as_i64),
        Some(7003)
    );

    // Pod container ports rewritten by name; the control port is left alone.
    let container_ports = gs
        .data
        .pointer("/spec/template/spec/containers/0/ports")
        .and_then(Value::as_array)
        .unwrap();
    let by_name = |name: &str| {
        container_ports
            .iter()
            .find(|p| p.get("name").and_then(Value::as_str) == Some(name))
            .and_then(|p| p.get("containerPort").and_then(Value::as_i64))
    };
    assert_eq!(by_name("game"), Some(7003));
    assert_eq!(by_name("messaging"), Some(7004));
    assert_eq!(
        by_name("control"),
        Some(i64::from(grizzly_control_api::CONTROL_PORT)),
        "control port must track the shared grizzly_control_api::CONTROL_PORT"
    );

    // Env injected with the leased numbers as strings.
    let env = gs
        .data
        .pointer("/spec/template/spec/containers/0/env")
        .and_then(Value::as_array)
        .unwrap();
    let env_val = |key: &str| {
        env.iter()
            .find(|e| e.get("name").and_then(Value::as_str) == Some(key))
            .and_then(|e| e.get("value").and_then(Value::as_str))
    };
    assert_eq!(env_val("SERVERGAMEPORT"), Some("7003"));
    assert_eq!(env_val("SUPERVISOR_GAME_PORT"), Some("7003"));
    assert_eq!(env_val("SERVERMESSAGINGPORT"), Some("7004"));
}

#[test]
fn pvc_takes_instance_data_name() {
    let pvc = render_pvc(&entry(), &identity()).unwrap();
    assert_eq!(pvc.metadata.name.as_deref(), Some("minecraft-ab12-data"));
    let labels = pvc.metadata.labels.as_ref().unwrap();
    assert_eq!(labels.get(INSTANCE_KEY).unwrap(), "minecraft-ab12");
}

#[test]
fn gameserver_without_claim_volume_is_rejected() {
    let mut broken = entry();
    broken.gameserver_yaml = "
apiVersion: agones.dev/v1
kind: GameServer
metadata:
  name: minecraft
spec:
  template:
    spec:
      volumes:
        - name: scratch
          emptyDir: {}
"
    .to_owned();
    let err = render_gameserver(&broken, &identity()).unwrap_err();
    assert!(
        err.to_string().contains("persistentVolumeClaim"),
        "should explain the missing claim volume, got: {err}"
    );
}

#[test]
fn service_without_ports_is_rejected() {
    let mut broken = entry();
    broken.service_yaml = "
apiVersion: v1
kind: Service
metadata:
  name: minecraft
spec:
  type: NodePort
"
    .to_owned();
    let err = render_service(&broken, &identity()).unwrap_err();
    assert!(
        err.to_string().contains("no ports"),
        "should explain the missing ports, got: {err}"
    );
}
