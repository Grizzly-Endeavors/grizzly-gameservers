use super::*;

use k8s_openapi::api::core::v1::{PersistentVolumeClaimSpec, PersistentVolumeClaimStatus, PodSpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

fn quantity_map(entries: &[(&str, &str)]) -> BTreeMap<String, Quantity> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), Quantity((*value).to_string())))
        .collect()
}

fn labelled_pvc(
    instance: &str,
    capacity: Option<&str>,
    request: Option<&str>,
) -> PersistentVolumeClaim {
    let labels: BTreeMap<String, String> = [
        (MANAGED_BY_KEY, MANAGED_BY_VALUE),
        (INSTANCE_KEY, instance),
        (GAME_KEY, "minecraft"),
        (GUILD_KEY, "42"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect();
    PersistentVolumeClaim {
        metadata: ObjectMeta {
            name: Some(format!("{instance}-data")),
            labels: Some(labels),
            ..ObjectMeta::default()
        },
        spec: request.map(|value| PersistentVolumeClaimSpec {
            resources: Some(k8s_openapi::api::core::v1::VolumeResourceRequirements {
                requests: Some(quantity_map(&[("storage", value)])),
                ..Default::default()
            }),
            ..Default::default()
        }),
        status: capacity.map(|value| PersistentVolumeClaimStatus {
            capacity: Some(quantity_map(&[("storage", value)])),
            ..Default::default()
        }),
    }
}

fn game_pod(instance: &str, containers: Vec<Container>) -> Pod {
    let labels: BTreeMap<String, String> =
        [(GAMESERVER_SELECTOR_KEY.to_string(), instance.to_string())]
            .into_iter()
            .collect();
    Pod {
        metadata: ObjectMeta {
            name: Some(instance.to_string()),
            labels: Some(labels),
            ..ObjectMeta::default()
        },
        spec: Some(PodSpec {
            containers,
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn container(requests: &[(&str, &str)], limits: &[(&str, &str)]) -> Container {
    Container {
        resources: Some(ResourceRequirements {
            requests: (!requests.is_empty()).then(|| quantity_map(requests)),
            limits: (!limits.is_empty()).then(|| quantity_map(limits)),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[test]
fn cpu_quantities_parse_across_the_suffixes() {
    // metrics-server nanocores, spec millicores, bare and fractional cores.
    assert_eq!(parse_cpu_millicores("3578906n"), Some(4));
    assert_eq!(parse_cpu_millicores("500m"), Some(500));
    assert_eq!(parse_cpu_millicores("2"), Some(2000));
    assert_eq!(parse_cpu_millicores("0.5"), Some(500));
    assert_eq!(parse_cpu_millicores("1500u"), Some(2));
    assert_eq!(parse_cpu_millicores(" 250m "), Some(250));
    assert_eq!(parse_cpu_millicores(""), None);
    assert_eq!(parse_cpu_millicores("100x"), None);
    assert_eq!(parse_cpu_millicores("1.2.3"), None);
    assert_eq!(parse_cpu_millicores("-500m"), None);
}

#[test]
fn byte_quantities_parse_across_the_suffixes() {
    assert_eq!(parse_bytes("2Gi"), Some(2_147_483_648));
    assert_eq!(parse_bytes("1536Ki"), Some(1_572_864));
    assert_eq!(parse_bytes("5G"), Some(5_000_000_000));
    assert_eq!(parse_bytes("128974848"), Some(128_974_848));
    assert_eq!(parse_bytes("1.5Gi"), Some(1_610_612_736));
    // The kubelet's cursed millibytes round to whole bytes.
    assert_eq!(parse_bytes("128974848000m"), Some(128_974_848));
    assert_eq!(parse_bytes("10Zi"), None);
    assert_eq!(parse_bytes("lots"), None);
}

#[test]
fn a_pod_backed_instance_gets_a_full_row_running_or_paused() {
    // Whether the game process is up or paused never enters this module: a pod
    // holding its reservation is sampled identically either way.
    let pvcs = [labelled_pvc("otter", Some("5Gi"), None)];
    let pods = [game_pod(
        "otter",
        vec![container(
            &[("cpu", "500m"), ("memory", "2Gi")],
            &[("cpu", "2"), ("memory", "4Gi")],
        )],
    )];
    let usage = [PodUsage {
        name: "otter".to_string(),
        cpu_millicores: Some(120),
        mem_bytes: Some(1_610_612_736),
    }];

    let samples = build_resource_samples(&pvcs, &pods, &usage);
    assert_eq!(samples.len(), 1);
    let sample = samples.first().expect("one sample");
    assert_eq!(sample.instance.as_str(), "otter");
    assert_eq!(sample.game.as_ref().map(GameId::as_str), Some("minecraft"));
    assert_eq!(sample.guild.as_ref().map(GuildId::as_str), Some("42"));
    assert_eq!(sample.cpu_millicores, Some(120));
    assert_eq!(sample.mem_bytes, Some(1_610_612_736));
    assert_eq!(sample.cpu_request_millicores, Some(500));
    assert_eq!(sample.mem_request_bytes, Some(2_147_483_648));
    assert_eq!(sample.cpu_limit_millicores, Some(2000));
    assert_eq!(sample.mem_limit_bytes, Some(4_294_967_296));
    assert_eq!(sample.pvc_bytes, Some(5_368_709_120));
}

#[test]
fn a_shut_down_instance_gets_a_pvc_only_row() {
    let pvcs = [labelled_pvc("otter", Some("5Gi"), None)];

    let samples = build_resource_samples(&pvcs, &[], &[]);
    assert_eq!(samples.len(), 1);
    let sample = samples.first().expect("one sample");
    assert_eq!(sample.pvc_bytes, Some(5_368_709_120));
    assert_eq!(sample.cpu_millicores, None);
    assert_eq!(sample.mem_bytes, None);
    assert_eq!(sample.cpu_request_millicores, None);
    assert_eq!(sample.mem_request_bytes, None);
    assert_eq!(sample.cpu_limit_millicores, None);
    assert_eq!(sample.mem_limit_bytes, None);
}

#[test]
fn a_metrics_miss_keeps_the_row_with_null_actuals() {
    let pvcs = [labelled_pvc("otter", Some("5Gi"), None)];
    let pods = [game_pod(
        "otter",
        vec![container(&[("cpu", "500m"), ("memory", "2Gi")], &[])],
    )];

    let samples = build_resource_samples(&pvcs, &pods, &[]);
    assert_eq!(samples.len(), 1);
    let sample = samples.first().expect("one sample");
    assert_eq!(sample.cpu_millicores, None);
    assert_eq!(sample.mem_bytes, None);
    // Declared figures survive: null actuals mean "no reading", not "no pod".
    assert_eq!(sample.cpu_request_millicores, Some(500));
    assert_eq!(sample.mem_request_bytes, Some(2_147_483_648));
}

#[test]
fn declared_figures_sum_over_containers_that_declare_them() {
    // Game container with requests+limits, Agones-style sidecar with requests
    // only: requests sum both, limits carry the game container's alone.
    let pvcs = [labelled_pvc("otter", Some("5Gi"), None)];
    let pods = [game_pod(
        "otter",
        vec![
            container(
                &[("cpu", "500m"), ("memory", "2Gi")],
                &[("cpu", "2"), ("memory", "4Gi")],
            ),
            container(&[("cpu", "30m"), ("memory", "64Mi")], &[]),
        ],
    )];

    let samples = build_resource_samples(&pvcs, &pods, &[]);
    let sample = samples.first().expect("one sample");
    assert_eq!(sample.cpu_request_millicores, Some(530));
    assert_eq!(sample.mem_request_bytes, Some(2_147_483_648 + 67_108_864));
    assert_eq!(sample.cpu_limit_millicores, Some(2000));
    assert_eq!(sample.mem_limit_bytes, Some(4_294_967_296));
}

#[test]
fn a_managed_pvc_without_an_instance_label_is_skipped() {
    let mut pvc = labelled_pvc("otter", Some("5Gi"), None);
    if let Some(labels) = pvc.metadata.labels.as_mut() {
        labels.remove(INSTANCE_KEY);
    }

    assert!(build_resource_samples(&[pvc], &[], &[]).is_empty());
}

#[test]
fn pvc_capacity_falls_back_to_the_spec_request_while_binding() {
    let pvcs = [labelled_pvc("otter", None, Some("5Gi"))];

    let samples = build_resource_samples(&pvcs, &[], &[]);
    let sample = samples.first().expect("one sample");
    assert_eq!(sample.pvc_bytes, Some(5_368_709_120));
}

#[test]
fn whole_pod_usage_sums_all_containers() {
    let usage = sum_parsed(["120m", "30m"].into_iter(), parse_cpu_millicores, "otter");
    assert_eq!(usage, Some(150));

    // An unparseable value is dropped, not zeroed — and no value at all is
    // unknown, not zero.
    let partial = sum_parsed(["120m", "junk"].into_iter(), parse_cpu_millicores, "otter");
    assert_eq!(partial, Some(120));
    let none = sum_parsed(std::iter::empty(), parse_cpu_millicores, "otter");
    assert_eq!(none, None);
}
