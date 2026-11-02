use super::*;

const MINECRAFT_GAMESERVER: &str = "\
apiVersion: agones.dev/v1
kind: GameServer
spec:
  container: minecraft
  template:
    spec:
      containers:
        - name: minecraft
          resources:
            requests:
              cpu: 500m
              memory: 2Gi
            limits:
              cpu: \"2\"
              memory: 4Gi
        - name: sidecar
          resources:
            requests:
              cpu: \"99\"
              memory: 99Gi
";

const MINECRAFT_PVC: &str = "\
apiVersion: v1
kind: PersistentVolumeClaim
spec:
  accessModes: [ReadWriteOnce]
  resources:
    requests:
      storage: 5Gi
";

fn entry(gameserver: &str, pvc: &str) -> GameCatalogEntry {
    GameCatalogEntry {
        id: "minecraft".to_owned(),
        gameserver_yaml: gameserver.to_owned(),
        service_yaml: String::new(),
        pvc_yaml: pvc.to_owned(),
        mod_registry: None,
        mods_file: None,
        env_overlay: None,
        config_rewritten_on_shutdown: None,
    }
}

#[test]
fn a_create_is_priced_from_the_catalog_plus_its_pvc() {
    let admission = create_admission(&entry(MINECRAFT_GAMESERVER, MINECRAFT_PVC), None);
    assert_eq!(admission.pvcs, 1);
    assert_eq!(admission.storage_bytes, 5 * 1024 * 1024 * 1024);
    // The game's own figures plus the injected sidecar's, and nothing from the
    // second container in the template — `spec.container` names which one is the
    // game, and the other is Agones' own.
    assert_eq!(
        admission.cpu_request_millicores,
        500 + SDK_SIDECAR.cpu_request_millicores
    );
    assert_eq!(
        admission.cpu_limit_millicores,
        2_000 + SDK_SIDECAR.cpu_limit_millicores
    );
}

#[test]
fn a_start_brings_no_pvc_or_storage() {
    let admission = start_admission(&entry(MINECRAFT_GAMESERVER, MINECRAFT_PVC), None);
    assert_eq!(admission.pvcs, 0);
    assert_eq!(admission.storage_bytes, 0);
    assert_eq!(
        admission.mem_limit_bytes,
        4 * 1024 * 1024 * 1024 + SDK_SIDECAR.mem_limit_bytes
    );
}

/// A resized instance renders with its override, so pricing it from the catalog
/// would under-count exactly the servers most likely to be near the ceiling.
#[test]
fn an_override_replaces_the_catalog_figures() {
    let sizing = Sizing {
        cpu_request_millicores: 4_000,
        mem_request_bytes: 16 * 1024 * 1024 * 1024,
        cpu_limit_millicores: 8_000,
        mem_limit_bytes: 32 * 1024 * 1024 * 1024,
    };
    let admission = start_admission(&entry(MINECRAFT_GAMESERVER, MINECRAFT_PVC), Some(sizing));
    assert_eq!(
        admission.cpu_limit_millicores,
        8_000 + SDK_SIDECAR.cpu_limit_millicores
    );
}

/// A malformed template must not become a namespace-wide refusal: it prices as
/// the sidecar alone, matching how `check_create` treats an unreadable sizing.
#[test]
fn an_unreadable_template_prices_as_the_sidecar_alone() {
    let admission = create_admission(&entry("nonsense: [", "also nonsense: ["), None);
    assert_eq!(
        admission.cpu_limit_millicores,
        SDK_SIDECAR.cpu_limit_millicores
    );
    assert_eq!(admission.storage_bytes, 0);
    // Still one PVC — the claim's *size* is unknown, but a create always makes one.
    assert_eq!(admission.pvcs, 1);
}

#[test]
fn the_tightest_of_several_quotas_is_the_one_that_binds() {
    let mut slot = None;
    tighten(
        &mut slot,
        Some(Dimension {
            hard: 100,
            used: 10,
        }),
    );
    tighten(&mut slot, Some(Dimension { hard: 50, used: 45 }));
    tighten(&mut slot, Some(Dimension { hard: 80, used: 20 }));
    assert_eq!(slot, Some(Dimension { hard: 50, used: 45 }));
}

#[test]
fn an_absent_dimension_leaves_the_slot_alone() {
    let mut slot = Some(Dimension { hard: 10, used: 1 });
    tighten(&mut slot, None);
    assert_eq!(slot, Some(Dimension { hard: 10, used: 1 }));
}
