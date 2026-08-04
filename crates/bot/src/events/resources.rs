//! The resource sampler: the cost time series of the product plane.
//!
//! Every tick it walks the managed instances and records what each one holds of
//! the hardware — actual whole-pod CPU and memory from metrics-server, the
//! declared requests and limits off the live pod spec, and the provisioned PVC
//! capacity. Integrating those rows over time gives GB-hours and core-hours per
//! instance, which the query layer prices against a market-proxy rate card
//! (`scripts/queries/`). The bot itself never computes money — the same boundary
//! the Langfuse plane holds for token pricing — so a rate revision reprices all
//! of history instead of being baked into rows.
//!
//! The sampling universe is the managed PVC list, not the running set, because
//! cost outlives the process: a paused instance still reserves its pod's RAM,
//! and a shut-down instance still holds its volume. Three row shapes follow,
//! and they are what the tests pin:
//!
//! - **Pod exists** (running *or* paused — this module never asks which): full
//!   row — actuals from metrics-server, declared from the pod spec, PVC
//!   capacity. Unlike the occupancy sampler, paused instances are not skipped.
//! - **Shut down** (no pod, PVC retained): PVC-only row — every compute column
//!   null, `pvc_bytes` set.
//! - **Destroyed**: no row at all; absence is how "gone" reads.
//!
//! A null quantity always means *that reading wasn't available this tick* —
//! metrics-server hadn't seen a young pod yet, or a listing failed — never zero.
//!
//! Declared resources come off the live pod spec rather than the catalog YAML:
//! the pod is what admission actually applied, and it includes the Agones SDK
//! sidecar's own footprint, which the catalog templates never mention. Both the
//! declared and actual figures are whole-pod sums so that the packing ratio
//! (actual ÷ requested) compares like with like.

use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::{Container, PersistentVolumeClaim, Pod, ResourceRequirements};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use kube::Client;
use kube::api::{Api, ListParams};
use kube::core::{ApiResource, DynamicObject, GroupVersionKind};
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use tracing::{debug, error, warn};

use super::sampler::SamplerCtx;
use crate::agones::{
    GAME_KEY, GAMESERVER_SELECTOR_KEY, GUILD_KEY, INSTANCE_KEY, MANAGED_BY_KEY, MANAGED_BY_VALUE,
    label_value,
};
use crate::domain::{GameId, GuildId, InstanceName};

/// One instance's resource footprint at one moment, bound for the
/// `resource_samples` table. Quantities only — millicores and bytes — priced
/// nowhere in the bot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResourceSample {
    pub(crate) instance: InstanceName,
    pub(crate) game: Option<GameId>,
    pub(crate) guild: Option<GuildId>,
    /// Actual whole-pod CPU usage in millicores, all containers summed
    /// (game + Agones sidecar — the sidecar exists only because the game does).
    pub(crate) cpu_millicores: Option<i64>,
    /// Actual whole-pod working-set memory in bytes.
    pub(crate) mem_bytes: Option<i64>,
    /// Declared CPU requests in millicores, summed over the containers that
    /// declare one (a requestless container reserves nothing).
    pub(crate) cpu_request_millicores: Option<i64>,
    /// Declared memory requests in bytes, summed like the CPU requests.
    pub(crate) mem_request_bytes: Option<i64>,
    /// Declared CPU limits in millicores, summed over the containers that
    /// declare one. The Agones sidecar typically declares none, so this is in
    /// practice the game container's burst envelope, not a pod-wide cap.
    pub(crate) cpu_limit_millicores: Option<i64>,
    /// Declared memory limits in bytes, summed like the CPU limits.
    pub(crate) mem_limit_bytes: Option<i64>,
    /// Provisioned volume capacity in bytes (`status.capacity`, falling back to
    /// the spec request while a claim is still binding).
    pub(crate) pvc_bytes: Option<i64>,
}

/// The unlabelled managed PVCs already reported, so the anomaly is stated once
/// per name instead of on every tick.
///
/// A managed PVC with no instance label never grows one on its own — it takes a
/// human deleting the volume or relabelling it. Re-warning every tick would
/// therefore add no information while burying everything else in the log, so the
/// sampler states it on first sight and stays quiet after. Process-lifetime
/// state: a restart re-reports, which is what makes the condition visible to
/// whoever is looking at a fresh log.
#[derive(Default)]
pub(crate) struct ReportedUnlabelledPvcs(HashSet<String>);

impl ReportedUnlabelledPvcs {
    /// True the first time `name` is seen — the caller warns only then.
    fn first_sighting(&mut self, name: &str) -> bool {
        self.0.insert(name.to_owned())
    }
}

/// Parsed actual usage for one pod, as reported by metrics-server.
struct PodUsage {
    name: String,
    cpu_millicores: Option<i64>,
    mem_bytes: Option<i64>,
}

/// The non-metadata payload of a `PodMetrics` item. Typed out rather than
/// walked as `serde_json::Value` so a schema surprise fails loudly in one
/// place instead of yielding silent nulls.
#[derive(Deserialize)]
struct PodMetricsData {
    containers: Vec<ContainerMetrics>,
}

#[derive(Deserialize)]
struct ContainerMetrics {
    usage: ContainerUsage,
}

#[derive(Deserialize)]
struct ContainerUsage {
    cpu: String,
    memory: String,
}

/// Sample every managed instance's resource footprint once and append the
/// tick, returning the rows so the watchdog detector can fold the tick in.
/// Never fails: without the PVC list there is no universe so the tick is
/// skipped; a failed pod or metrics listing degrades the affected columns to
/// null; the insert swallows its own errors.
pub(crate) async fn run_resource_cycle(
    ctx: &SamplerCtx<'_>,
    reported: &mut ReportedUnlabelledPvcs,
) -> Vec<ResourceSample> {
    let pvcs = match list_managed_pvcs(ctx.client, ctx.namespace).await {
        Ok(pvcs) => pvcs,
        Err(err) => {
            error!(error = ?err, "resource sampler could not list pvcs");
            return Vec::new();
        }
    };
    if pvcs.is_empty() {
        debug!("resource sampler tick: no managed instances");
        return Vec::new();
    }
    let pods = match list_game_pods(ctx.client, ctx.namespace).await {
        Ok(pods) => pods,
        Err(err) => {
            error!(
                error = ?err,
                "resource sampler: pod list failed; declared and actual columns null this tick"
            );
            Vec::new()
        }
    };
    let usage = match list_pod_usage(ctx.client, ctx.namespace).await {
        Ok(usage) => usage,
        Err(err) => {
            error!(
                error = ?err,
                "resource sampler: metrics list failed; actual columns null this tick"
            );
            Vec::new()
        }
    };

    let samples = build_resource_samples(&pvcs, &pods, &usage, reported);
    // Per-tick, so debug like the occupancy cycle's tick line.
    debug!(
        sampled = samples.len(),
        pods = pods.len(),
        "resource sampler tick"
    );
    ctx.events.record_resource_samples(&samples).await;
    samples
}

/// The per-instance PVCs the bot provisioned — the sampling universe, since the
/// volume exists exactly as long as the instance does.
async fn list_managed_pvcs(client: &Client, namespace: &str) -> Result<Vec<PersistentVolumeClaim>> {
    let api: Api<PersistentVolumeClaim> = Api::namespaced(client.clone(), namespace);
    let selector = format!("{MANAGED_BY_KEY}={MANAGED_BY_VALUE}");
    let list = api
        .list(&ListParams::default().labels(&selector))
        .await
        .with_context(|| format!("failed to list managed pvcs in namespace {namespace}"))?;
    Ok(list.items)
}

/// The live game pods, selected by the label Agones stamps on every pod it
/// creates. The bot's own pod shares the namespace but never carries it.
async fn list_game_pods(client: &Client, namespace: &str) -> Result<Vec<Pod>> {
    let api: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let list = api
        .list(&ListParams::default().labels(GAMESERVER_SELECTOR_KEY))
        .await
        .with_context(|| format!("failed to list game pods in namespace {namespace}"))?;
    Ok(list.items)
}

/// Actual usage per pod from metrics-server. `metrics.k8s.io` ships no typed
/// bindings in k8s-openapi, so this goes through `DynamicObject` the same way
/// the Agones `GameServer` access does.
async fn list_pod_usage(client: &Client, namespace: &str) -> Result<Vec<PodUsage>> {
    let gvk = GroupVersionKind::gvk("metrics.k8s.io", "v1beta1", "PodMetrics");
    // `from_gvk` would derive the plural from the kind; the API's real path
    // segment is `pods`.
    let resource = ApiResource::from_gvk_with_plural(&gvk, "pods");
    let api: Api<DynamicObject> = Api::namespaced_with(client.clone(), namespace, &resource);
    let list = api
        .list(&ListParams::default())
        .await
        .with_context(|| format!("failed to list pod metrics in namespace {namespace}"))?;
    Ok(list.items.into_iter().filter_map(pod_usage).collect())
}

/// Fold one `PodMetrics` item into a whole-pod usage reading, or into nothing
/// (with a warning) when the payload doesn't parse.
fn pod_usage(obj: DynamicObject) -> Option<PodUsage> {
    let name = obj.metadata.name.clone()?;
    let data: PodMetricsData = match serde_json::from_value(obj.data) {
        Ok(data) => data,
        Err(err) => {
            warn!(error = %err, pod = %name, "resource sampler: unparseable pod metrics payload");
            return None;
        }
    };
    let cpu_millicores = sum_parsed(
        data.containers.iter().map(|c| c.usage.cpu.as_str()),
        parse_cpu_millicores,
        &name,
    );
    let mem_bytes = sum_parsed(
        data.containers.iter().map(|c| c.usage.memory.as_str()),
        parse_bytes,
        &name,
    );
    Some(PodUsage {
        name,
        cpu_millicores,
        mem_bytes,
    })
}

/// Join the three listings into one row per managed PVC. Pure — everything
/// fallible happened in the listings, so this is where the tests live.
fn build_resource_samples(
    pvcs: &[PersistentVolumeClaim],
    pods: &[Pod],
    usage: &[PodUsage],
    reported: &mut ReportedUnlabelledPvcs,
) -> Vec<ResourceSample> {
    let mut samples = Vec::new();
    for pvc in pvcs {
        let labels = pvc.metadata.labels.as_ref();
        let Some(instance) = label_value(labels, INSTANCE_KEY) else {
            let name = pvc.metadata.name.as_deref().unwrap_or("(unnamed)");
            if reported.first_sighting(name) {
                warn!(
                    pvc = name,
                    "resource sampler: managed pvc missing its instance label; it is \
                     excluded from the cost plane until relabelled or deleted"
                );
            }
            continue;
        };
        // The pod is found through the label Agones stamps rather than by name,
        // and the metrics reading is then found through that pod — so an actual
        // can never attach to an instance whose pod wasn't seen this tick.
        let pod = pods.iter().find(|pod| {
            label_value(pod.metadata.labels.as_ref(), GAMESERVER_SELECTOR_KEY) == Some(instance)
        });
        let used = pod
            .and_then(|pod| pod.metadata.name.as_deref())
            .and_then(|pod_name| usage.iter().find(|reading| reading.name == pod_name));
        let declared = pod.map(declared_resources).unwrap_or_default();
        samples.push(ResourceSample {
            instance: InstanceName::new(instance),
            game: label_value(labels, GAME_KEY).map(GameId::new),
            guild: label_value(labels, GUILD_KEY).map(GuildId::new),
            cpu_millicores: used.and_then(|reading| reading.cpu_millicores),
            mem_bytes: used.and_then(|reading| reading.mem_bytes),
            cpu_request_millicores: declared.cpu_request_millicores,
            mem_request_bytes: declared.mem_request_bytes,
            cpu_limit_millicores: declared.cpu_limit_millicores,
            mem_limit_bytes: declared.mem_limit_bytes,
            pvc_bytes: pvc_capacity_bytes(pvc),
        });
    }
    samples
}

/// Whole-pod declared requests and limits, summed over the containers that
/// declare each figure.
#[derive(Default)]
struct DeclaredResources {
    cpu_request_millicores: Option<i64>,
    mem_request_bytes: Option<i64>,
    cpu_limit_millicores: Option<i64>,
    mem_limit_bytes: Option<i64>,
}

fn declared_resources(pod: &Pod) -> DeclaredResources {
    let containers: &[Container] = pod
        .spec
        .as_ref()
        .map_or(&[], |spec| spec.containers.as_slice());
    DeclaredResources {
        cpu_request_millicores: summed_quantity(
            containers,
            requests_of,
            "cpu",
            parse_cpu_millicores,
        ),
        mem_request_bytes: summed_quantity(containers, requests_of, "memory", parse_bytes),
        cpu_limit_millicores: summed_quantity(containers, limits_of, "cpu", parse_cpu_millicores),
        mem_limit_bytes: summed_quantity(containers, limits_of, "memory", parse_bytes),
    }
}

fn requests_of(resources: &ResourceRequirements) -> Option<&BTreeMap<String, Quantity>> {
    resources.requests.as_ref()
}

fn limits_of(resources: &ResourceRequirements) -> Option<&BTreeMap<String, Quantity>> {
    resources.limits.as_ref()
}

/// Sum one declared figure across a pod's containers; a container that doesn't
/// declare it contributes nothing, and no container declaring it means null.
fn summed_quantity(
    containers: &[Container],
    side: fn(&ResourceRequirements) -> Option<&BTreeMap<String, Quantity>>,
    key: &str,
    parse: fn(&str) -> Option<i64>,
) -> Option<i64> {
    let mut total: Option<i64> = None;
    for container in containers {
        let Some(quantity) = container
            .resources
            .as_ref()
            .and_then(side)
            .and_then(|map| map.get(key))
        else {
            continue;
        };
        if let Some(value) = parse(&quantity.0) {
            total = Some(total.unwrap_or(0).saturating_add(value));
        } else {
            warn!(
                quantity = %quantity.0,
                key,
                "resource sampler: unparseable declared quantity"
            );
        }
    }
    total
}

/// Sum already-attributed usage strings for one pod, tolerating (and warning
/// on) individually unparseable values.
fn sum_parsed<'a>(
    values: impl Iterator<Item = &'a str>,
    parse: fn(&str) -> Option<i64>,
    pod: &str,
) -> Option<i64> {
    let mut total: Option<i64> = None;
    for value in values {
        if let Some(parsed) = parse(value) {
            total = Some(total.unwrap_or(0).saturating_add(parsed));
        } else {
            warn!(quantity = %value, pod = %pod, "resource sampler: unparseable usage quantity");
        }
    }
    total
}

/// A PVC's provisioned capacity in bytes: what the cluster actually bound
/// (`status.capacity`), or the spec request while the claim is still binding.
fn pvc_capacity_bytes(pvc: &PersistentVolumeClaim) -> Option<i64> {
    let bound = pvc
        .status
        .as_ref()
        .and_then(|status| status.capacity.as_ref())
        .and_then(|capacity| capacity.get("storage"));
    let requested = pvc
        .spec
        .as_ref()
        .and_then(|spec| spec.resources.as_ref())
        .and_then(|resources| resources.requests.as_ref())
        .and_then(|requests| requests.get("storage"));
    let quantity = bound.or(requested)?;
    let parsed = parse_bytes(&quantity.0);
    if parsed.is_none() {
        warn!(
            quantity = %quantity.0,
            pvc = pvc.metadata.name.as_deref().unwrap_or("(unnamed)"),
            "resource sampler: unparseable pvc capacity"
        );
    }
    parsed
}

/// A Kubernetes CPU quantity in millicores: metrics-server reports nanocores
/// (`3578906n`), pod specs say millicores (`500m`) or bare cores (`2`, `0.5`).
fn parse_cpu_millicores(value: &str) -> Option<i64> {
    let (digits, frac_len, suffix) = split_quantity(value.trim())?;
    let (mult, div) = match suffix {
        "n" => (1, 1_000_000),
        "u" => (1, 1_000),
        "m" => (1, 1),
        "" => (1_000, 1),
        _ => return None,
    };
    scaled(digits, frac_len, mult, div)
}

/// A Kubernetes byte quantity in bytes: binary suffixes (`2Gi`), decimal
/// suffixes (`5G`), bare integers, and the kubelet's occasional millibytes.
fn parse_bytes(value: &str) -> Option<i64> {
    let (digits, frac_len, suffix) = split_quantity(value.trim())?;
    let (mult, div) = match suffix {
        "" => (1, 1),
        "m" => (1, 1_000),
        "k" => (1_000, 1),
        "M" => (1_000_000, 1),
        "G" => (1_000_000_000, 1),
        "T" => (1_000_000_000_000, 1),
        "P" => (1_000_000_000_000_000, 1),
        "E" => (1_000_000_000_000_000_000, 1),
        "Ki" => (1_i128 << 10, 1),
        "Mi" => (1_i128 << 20, 1),
        "Gi" => (1_i128 << 30, 1),
        "Ti" => (1_i128 << 40, 1),
        "Pi" => (1_i128 << 50, 1),
        "Ei" => (1_i128 << 60, 1),
        _ => return None,
    };
    scaled(digits, frac_len, mult, div)
}

/// Split a quantity into its digits (decimal point removed), the count of
/// fractional digits, and the unit suffix. Integer-only on purpose: the values
/// land in `BIGINT` columns, and rational math avoids both float rounding and
/// the cast lints.
fn split_quantity(value: &str) -> Option<(i128, u32, &str)> {
    let boundary = value
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(value.len());
    let (number, suffix) = value.split_at(boundary);
    let (int_part, frac_part) = number.split_once('.').unwrap_or((number, ""));
    if int_part.is_empty() && frac_part.is_empty() {
        return None;
    }
    let mut digits: i128 = 0;
    for c in int_part.chars().chain(frac_part.chars()) {
        // A second '.' lands here and correctly rejects the value.
        let digit = c.to_digit(10)?;
        digits = digits.checked_mul(10)?.checked_add(i128::from(digit))?;
    }
    let frac_len = u32::try_from(frac_part.len()).ok()?;
    Some((digits, frac_len, suffix))
}

/// `digits / 10^frac_len * mult / div`, rounded half-up, refused on overflow.
fn scaled(digits: i128, frac_len: u32, mult: i128, div: i128) -> Option<i64> {
    let numerator = digits.checked_mul(mult)?;
    let denominator = 10_i128.checked_pow(frac_len)?.checked_mul(div)?;
    let rounded = numerator
        .checked_add(denominator / 2)?
        .checked_div(denominator)?;
    i64::try_from(rounded).ok()
}

#[cfg(test)]
#[path = "tests/resources.rs"]
mod tests;
