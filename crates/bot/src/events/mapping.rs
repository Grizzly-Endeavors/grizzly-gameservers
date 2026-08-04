//! Outcome enums and payloads → the `outcome` and `details` columns.
//!
//! Every mapper here is pure and exhaustive, which is what makes the outcome
//! vocabulary safe to query: `wildcard_enum_match_arm` is denied workspace-wide,
//! so a new variant on any orchestration outcome breaks this file rather than
//! silently landing in the table as an unmapped row.
//!
//! One vocabulary, read across all kinds: the happy path names the action that
//! happened (`created`, `stopped`, `archived`), a refusal names why it couldn't
//! (`name_taken`, `ports_exhausted`, `not_managed`), and [`FAILED`] is the single
//! name for an `Err` the caller couldn't classify.

use std::time::Duration;

use serde_json::{Value, json};

use crate::agones::{
    DestroyOutcome, ProvisionOutcome, ResizeOutcome, ShutdownOutcome, StartBegin, SupervisorOutcome,
};
use crate::backup::{
    ArchiveOutcome, BackupOutcome, BootState, RecoverOutcome, RestoreOutcome, SafetyBackup,
};
use crate::domain::{GuildId, InstanceName};
use crate::limits::LimitRefusal;
use crate::notify::{Escalation, EscalationContext};
use crate::watchdog::{Incident, IncidentFacts};

use super::catalog::{Attribution, EventKind, ProductEvent};

/// The outcome for an action that returned `Err` — a cluster or internal fault
/// the caller had no classified value for.
pub(crate) const FAILED: &str = "failed";

/// The outcome for a scheduled cycle that finished a pass. A cycle has no failure
/// outcome of its own: per-server failures are counted in the details.
pub(crate) const COMPLETED: &str = "completed";

/// The outcome for a first-ever guild sighting.
pub(crate) const JOINED: &str = "joined";

/// An archive was deleted on request.
pub(crate) const DELETED: &str = "deleted";

/// An archive was deleted by the retention cycle rather than by a person, which
/// is the difference between "someone decided this world was finished with" and
/// "it aged out". Separable so an unexpected disappearance can be attributed.
pub(crate) const PRUNED: &str = "pruned";

/// Gary spent his whole round budget on a request without resolving it.
pub(crate) const ROUND_BUDGET_EXHAUSTED: &str = "round_budget_exhausted";

/// Gary called the escalation tool: he stopped and asked for a human while he
/// still had budget left. Separable from [`ROUND_BUDGET_EXHAUSTED`] because the
/// two say opposite things about how the agent is behaving — deliberate handoff
/// is the guardrail working, budget exhaustion is thrashing.
pub(crate) const AGENT_REQUESTED: &str = "agent_requested";

/// An automatic crash rollback didn't bring the server back, or couldn't be
/// issued at all.
pub(crate) const CRASH_ROLLBACK: &str = "crash_rollback";

/// How an in-place resize landed. Separable because "the setting is saved" and
/// "the server is running on it" are different states, and only the first is
/// guaranteed — the kubelet can decline a live change it can't action.
pub(crate) fn resize(outcome: &ResizeOutcome) -> &'static str {
    match *outcome {
        ResizeOutcome::Applied => "applied",
        ResizeOutcome::InProgress => "in_progress",
        // A stopped instance and an absent pod are the same story for the record:
        // saved, waiting for a start.
        ResizeOutcome::NoPod | ResizeOutcome::NotFound | ResizeOutcome::NotManaged => {
            "recorded_for_next_start"
        }
        ResizeOutcome::Infeasible(_) => "infeasible",
    }
}

/// A watchdog incident was detected in shadow mode: recorded, nothing acted.
pub(crate) const SHADOW: &str = "shadow";

/// A watchdog incident ran a proactive Gary turn.
pub(crate) const RESPONDED: &str = "responded";

/// A watchdog incident was suppressed by the storm guard — too many proactive
/// turns in the window, so operators got a one-line DM instead.
pub(crate) const SUPPRESSED_STORM: &str = "suppressed_storm";

/// A watchdog crash incident was left to an armed deferred `startup` watcher,
/// which runs its own investigation batch for the same event.
pub(crate) const DEFERRED_TO_BATCH: &str = "deferred_to_batch";

/// A watchdog incident had nowhere to go — no gateway, or no home channel and
/// no operator DM could be opened.
pub(crate) const UNDELIVERABLE: &str = "undeliverable";

/// A watchdog incident's proactive turn errored before producing a reply.
pub(crate) const TURN_FAILED: &str = "turn_failed";

/// Which way Gary gave up. The escalation kind *is* the outcome, so the two
/// escalation shapes are separable without reaching into the details payload.
pub(crate) fn escalation(escalation: &Escalation) -> &'static str {
    match escalation {
        Escalation::AgentRequested { .. } => AGENT_REQUESTED,
        Escalation::RoundBudgetExhausted { .. } => ROUND_BUDGET_EXHAUSTED,
        Escalation::CrashRollback { .. } => CRASH_ROLLBACK,
    }
}

/// The row for an escalation, with whichever locators its kind carries — a
/// Discord give-up has neither a server nor a file, an in-game or deferred one
/// names its server, an agent-requested one names whichever server the model
/// said it was about, and a crash rollback names both.
pub(crate) fn escalation_event(
    escalation: &Escalation,
    attribution: &Attribution,
    guild: Option<GuildId>,
) -> ProductEvent {
    let (server, path) = match escalation {
        // The model names the server itself, which is the only locator a Discord
        // handoff has — so it wins over the context, which carries none.
        Escalation::AgentRequested {
            server: Some(server),
            ..
        } => (Some(server.as_str()), None),
        Escalation::AgentRequested { context, .. }
        | Escalation::RoundBudgetExhausted { context, .. } => match context {
            EscalationContext::Discord { .. } => (None, None),
            EscalationContext::InGame { server, .. }
            | EscalationContext::Deferred { server, .. }
            | EscalationContext::Watchdog { server, .. } => (Some(server.as_str()), None),
        },
        Escalation::CrashRollback { server, path } => (Some(server.as_str()), Some(path.as_str())),
    };
    let mut event = ProductEvent::new(
        EventKind::Escalation,
        attribution,
        self::escalation(escalation),
    )
    .guild(guild)
    .details(escalation_details(server, path));
    if let Some(server) = server {
        event = event.instance(InstanceName::new(server));
    }
    event
}

/// The row for a watchdog incident: which signal fired, its numbers, and where
/// the response went (`delivery` is null when nothing was delivered — shadow,
/// suppressed, undeliverable).
pub(crate) fn incident_event(
    incident: &Incident,
    outcome: &'static str,
    delivery: Option<&str>,
) -> ProductEvent {
    ProductEvent::new(EventKind::Incident, &Attribution::system(), outcome)
        .guild(incident.guild.clone())
        .game(incident.game.clone())
        .instance(incident.instance.clone())
        .details(incident_details(incident, delivery))
}

/// Payload for a watchdog incident: the signal name plus its kind-specific
/// numbers, flattened so SQL reaches each figure without knowing the kind.
fn incident_details(incident: &Incident, delivery: Option<&str>) -> Value {
    let facts = match &incident.facts {
        IncidentFacts::CrashLoop {
            restarts_delta,
            window_minutes,
            total_restarts,
            uptime_seconds,
        } => json!({
            "restarts_delta": restarts_delta,
            "window_minutes": window_minutes,
            "total_restarts": total_restarts,
            "uptime_seconds": uptime_seconds,
        }),
        IncidentFacts::Crashed { total_restarts } => json!({
            "total_restarts": total_restarts,
        }),
        IncidentFacts::Unreachable { minutes } => json!({
            "unreachable_minutes": minutes,
        }),
        // The supervisor that held the numbers died with the pod.
        IncidentFacts::Unhealthy => json!({}),
        IncidentFacts::MemoryPressure {
            used_bytes,
            limit_bytes,
            percent,
        } => json!({
            "mem_used_bytes": used_bytes,
            "mem_limit_bytes": limit_bytes,
            "mem_percent": percent,
        }),
    };
    let mut details = serde_json::Map::new();
    details.insert("signal".to_owned(), json!(incident.kind.as_str()));
    details.insert("delivery".to_owned(), json!(delivery));
    if let Value::Object(fields) = facts {
        details.extend(fields);
    }
    Value::Object(details)
}

/// How the provisioning phase of a create ended. A successful provision is not
/// yet terminal — the caller resolves readiness and reports [`readiness`]
/// instead, so `provisioned` only ever lands when readiness was never awaited.
pub(crate) fn provision(outcome: &ProvisionOutcome) -> &'static str {
    match outcome {
        ProvisionOutcome::Provisioned { .. } => "provisioned",
        ProvisionOutcome::AlreadyExists => "name_taken",
        ProvisionOutcome::PortsExhausted => "ports_exhausted",
        ProvisionOutcome::GuildLimitReached(refusal) => limit_refusal(*refusal),
    }
}

/// The outcome name for a guild-cap refusal (ADR-022) — split by which cap, so
/// "too many servers" and "too much CPU/memory" stay separately countable.
pub(crate) fn limit_refusal(refusal: LimitRefusal) -> &'static str {
    match refusal {
        LimitRefusal::ServerCount { .. } => "guild_limit_servers",
        LimitRefusal::AggregateCpu { .. } | LimitRefusal::AggregateMemory { .. } => {
            "guild_limit_resources"
        }
    }
}

/// How the start-up phase of a cold start ended. As with [`provision`], a
/// successful start is resolved by [`readiness`].
pub(crate) fn start_begin(outcome: &StartBegin) -> &'static str {
    match outcome {
        StartBegin::Starting { .. } => "starting",
        StartBegin::AlreadyRunning => "already_running",
        StartBegin::NotFound => "not_found",
        StartBegin::NotManaged => "not_managed",
        StartBegin::UnknownGame(_) => "unknown_game",
    }
}

/// The terminal outcome of a create or cold start, once the readiness wait has
/// resolved. `unconfirmed` means the wait elapsed with the server still coming
/// up — the objects exist, but nobody can connect yet.
pub(crate) fn readiness(ready: bool) -> &'static str {
    if ready { "ready" } else { "unconfirmed" }
}

/// How a warm supervisor action ended. The event kind already says which action
/// was asked for, so the three success variants map to their own past tense.
pub(crate) fn supervisor(outcome: &SupervisorOutcome) -> &'static str {
    match outcome {
        SupervisorOutcome::Paused => "stopped",
        SupervisorOutcome::Resumed => "started",
        SupervisorOutcome::Restarted => "restarted",
        SupervisorOutcome::AlreadyStopped => "already_stopped",
        SupervisorOutcome::AlreadyRunning => "already_running",
        SupervisorOutcome::PodNotReady => "pod_not_ready",
        SupervisorOutcome::Unreachable => "unreachable",
        SupervisorOutcome::Failed(_) => FAILED,
        SupervisorOutcome::NotFound => "not_found",
        SupervisorOutcome::NotManaged => "not_managed",
    }
}

pub(crate) fn shutdown(outcome: &ShutdownOutcome) -> &'static str {
    match outcome {
        ShutdownOutcome::Down => "down",
        ShutdownOutcome::Stranded => "stranded",
        ShutdownOutcome::NotFound => "not_found",
        ShutdownOutcome::NotManaged => "not_managed",
    }
}

pub(crate) fn destroy(outcome: &DestroyOutcome) -> &'static str {
    match outcome {
        DestroyOutcome::Destroyed => "destroyed",
        DestroyOutcome::Blocked => "blocked",
        DestroyOutcome::NotFound => "not_found",
        DestroyOutcome::NotManaged => "not_managed",
    }
}

pub(crate) fn backup(outcome: &BackupOutcome) -> &'static str {
    match outcome {
        BackupOutcome::BackedUp { .. } => "backed_up",
        BackupOutcome::NotFound => "not_found",
        BackupOutcome::NotManaged => "not_managed",
        BackupOutcome::NotRunning => "not_running",
        BackupOutcome::Unreachable(_) => "unreachable",
    }
}

pub(crate) fn archive(outcome: &ArchiveOutcome) -> &'static str {
    match outcome {
        ArchiveOutcome::Archived { .. } => "archived",
        ArchiveOutcome::ArchivedNotReleased { .. } => "archived_not_released",
        ArchiveOutcome::NotFound => "not_found",
        ArchiveOutcome::NotManaged => "not_managed",
        ArchiveOutcome::Unavailable => "unavailable",
        ArchiveOutcome::Failed(_) => FAILED,
    }
}

pub(crate) fn restore(outcome: &RestoreOutcome) -> &'static str {
    match outcome {
        RestoreOutcome::Restored { .. } => "restored",
        RestoreOutcome::NotFound => "not_found",
        RestoreOutcome::NotManaged => "not_managed",
        RestoreOutcome::Failed(_) => FAILED,
    }
}

pub(crate) fn recover(outcome: &RecoverOutcome) -> &'static str {
    match outcome {
        RecoverOutcome::Recovered { .. } => "recovered",
        RecoverOutcome::NoSuchArchive => "no_such_archive",
        RecoverOutcome::NameInUse => "name_in_use",
        RecoverOutcome::Unavailable => "unavailable",
        RecoverOutcome::UnknownGame(_) => "unknown_game",
        RecoverOutcome::PortsExhausted => "ports_exhausted",
        RecoverOutcome::GuildLimitReached(refusal) => limit_refusal(*refusal),
        RecoverOutcome::Failed(_) => FAILED,
    }
}

fn boot_state(boot: BootState) -> &'static str {
    match boot {
        BootState::Ready => "ready",
        BootState::Crashed => "crashed",
        BootState::Stopped => "stopped",
        BootState::TimedOut => "timed_out",
    }
}

fn safety_backup(safety: SafetyBackup) -> &'static str {
    match safety {
        SafetyBackup::Taken => "taken",
        SafetyBackup::Absent => "absent",
    }
}

/// Milliseconds, saturating — `serde_json` has no `u128`, and a duration that
/// overflows `u64` milliseconds is half a billion years.
fn millis(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

/// Payload for a create or cold start. `time_to_ready_ms` is null whenever
/// readiness wasn't confirmed, never zero — an unconfirmed boot is unknown, not
/// instant.
pub(crate) fn boot_details(address: Option<&str>, time_to_ready: Option<Duration>) -> Value {
    json!({
        "address": address,
        "time_to_ready_ms": time_to_ready.map(millis),
    })
}

/// Payload for a backup or an archive.
pub(crate) fn size_details(size_bytes: u64) -> Value {
    json!({ "size_bytes": size_bytes })
}

/// Payload for a recover: the server is new, so it reports where it landed and
/// how it came up.
pub(crate) fn recover_details(address: &str, boot: BootState) -> Value {
    json!({ "address": address, "boot": boot_state(boot) })
}

/// Payload for a restore: how the server came back, and whether the overwrite
/// left an undo point.
pub(crate) fn restore_details(boot: BootState, safety: SafetyBackup) -> Value {
    json!({ "boot": boot_state(boot), "safety_backup": safety_backup(safety) })
}

/// Payload for a scheduled backup pass.
pub(crate) fn cycle_details(servers: usize, succeeded: usize, failed: usize) -> Value {
    json!({ "servers": servers, "succeeded": succeeded, "failed": failed })
}

/// Payload for an escalation. Both locators are null for a Discord give-up,
/// which has neither.
pub(crate) fn escalation_details(server: Option<&str>, path: Option<&str>) -> Value {
    json!({ "server": server, "path": path })
}

#[cfg(test)]
#[path = "tests/mapping.rs"]
mod tests;
