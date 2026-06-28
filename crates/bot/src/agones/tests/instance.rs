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
        name: "minecraft-ab12".to_owned(),
        game: "minecraft".to_owned(),
        namespace: "game-servers".to_owned(),
        node_port: 7003,
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
