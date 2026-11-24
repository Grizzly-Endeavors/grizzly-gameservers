use std::time::Duration;

use serde_json::{Value, json};

use crate::agones::{
    DestroyOutcome, ProvisionOutcome, ResizeOutcome, ShutdownOutcome, StartBegin, SupervisorOutcome,
};
use crate::backup::{
    ArchiveOutcome, BackupOutcome, BootState, RecoverOutcome, RestoreOutcome, SafetyBackup,
};
use crate::capacity::{CapacityRefusal, QuotaBasis};
use crate::domain::{GuildId, InstanceName};
use crate::limits::LimitRefusal;
use crate::notify::{Escalation, EscalationContext};
use crate::watchdog::{Incident, IncidentFacts};

use super::catalog::{Attribution, EventKind, ProductEvent};

pub(crate) const FAILED: &str = "failed";

pub(crate) const COMPLETED: &str = "completed";

pub(crate) const JOINED: &str = "joined";

pub(crate) const CHANGED: &str = "changed";

pub(crate) const DELETED: &str = "deleted";

pub(crate) const PRUNED: &str = "pruned";

pub(crate) const ROUND_BUDGET_EXHAUSTED: &str = "round_budget_exhausted";

pub(crate) const AGENT_REQUESTED: &str = "agent_requested";

pub(crate) const CRASH_ROLLBACK: &str = "crash_rollback";

pub(crate) fn resize(outcome: &ResizeOutcome) -> &'static str {
    match *outcome {
        ResizeOutcome::Applied => "applied",
        ResizeOutcome::InProgress => "in_progress",
        ResizeOutcome::NoPod | ResizeOutcome::NotFound | ResizeOutcome::NotManaged => {
            "recorded_for_next_start"
        }
        ResizeOutcome::Infeasible(_) => "infeasible",
    }
}

pub(crate) const SHADOW: &str = "shadow";

pub(crate) const RESPONDED: &str = "responded";

pub(crate) const SUPPRESSED_STORM: &str = "suppressed_storm";

pub(crate) const DEFERRED_TO_BATCH: &str = "deferred_to_batch";

pub(crate) const UNDELIVERABLE: &str = "undeliverable";

pub(crate) const TURN_FAILED: &str = "turn_failed";

pub(crate) const STOPPED: &str = "stopped";

pub(crate) fn escalation(escalation: &Escalation) -> &'static str {
    match escalation {
        Escalation::AgentRequested { .. } => AGENT_REQUESTED,
        Escalation::RoundBudgetExhausted { .. } => ROUND_BUDGET_EXHAUSTED,
        Escalation::CrashRollback { .. } => CRASH_ROLLBACK,
    }
}

pub(crate) fn escalation_event(
    escalation: &Escalation,
    attribution: &Attribution,
    guild: Option<GuildId>,
) -> ProductEvent {
    let (server, path) = match escalation {
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

pub(crate) fn turn_cancelled_event(
    attribution: &Attribution,
    guild: Option<GuildId>,
) -> ProductEvent {
    ProductEvent::new(EventKind::TurnCancelled, attribution, STOPPED).guild(guild)
}

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

pub(crate) fn provision(outcome: &ProvisionOutcome) -> &'static str {
    match outcome {
        ProvisionOutcome::Provisioned { .. } => "provisioned",
        ProvisionOutcome::AlreadyExists => "name_taken",
        ProvisionOutcome::NameUnavailable => "name_held_by_another_guild",
        ProvisionOutcome::PortsExhausted => "ports_exhausted",
        ProvisionOutcome::GuildLimitReached(refusal) => limit_refusal(*refusal),
        ProvisionOutcome::ClusterFull(refusal) => capacity_refusal(*refusal),
    }
}

pub(crate) fn capacity_refusal(refusal: CapacityRefusal) -> &'static str {
    match refusal {
        CapacityRefusal::Servers { .. } => "cluster_full_servers",
        CapacityRefusal::Storage { .. } => "cluster_full_storage",
        CapacityRefusal::Cpu {
            basis: QuotaBasis::Requests,
            ..
        } => "cluster_full_cpu_requests",
        CapacityRefusal::Cpu {
            basis: QuotaBasis::Limits,
            ..
        } => "cluster_full_cpu_limits",
        CapacityRefusal::Memory {
            basis: QuotaBasis::Requests,
            ..
        } => "cluster_full_memory_requests",
        CapacityRefusal::Memory {
            basis: QuotaBasis::Limits,
            ..
        } => "cluster_full_memory_limits",
        CapacityRefusal::Unspecified => "cluster_full",
    }
}

pub(crate) fn limit_refusal(refusal: LimitRefusal) -> &'static str {
    match refusal {
        LimitRefusal::ServerCount { .. } => "guild_limit_servers",
        LimitRefusal::AggregateCpu { .. } | LimitRefusal::AggregateMemory { .. } => {
            "guild_limit_resources"
        }
    }
}

pub(crate) fn start_begin(outcome: &StartBegin) -> &'static str {
    match outcome {
        StartBegin::Starting { .. } => "starting",
        StartBegin::AlreadyRunning => "already_running",
        StartBegin::NotFound => "not_found",
        StartBegin::NotManaged => "not_managed",
        StartBegin::UnknownGame(_) => "unknown_game",
        StartBegin::ClusterFull(refusal) => capacity_refusal(*refusal),
    }
}

pub(crate) fn readiness(ready: bool) -> &'static str {
    if ready { "ready" } else { "unconfirmed" }
}

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
        RecoverOutcome::ClusterFull(refusal) => capacity_refusal(*refusal),
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

pub(crate) fn boot_details(address: Option<&str>, time_to_ready: Option<Duration>) -> Value {
    json!({
        "address": address,
        "time_to_ready_ms": time_to_ready.map(millis),
    })
}

pub(crate) fn size_details(size_bytes: u64) -> Value {
    json!({ "size_bytes": size_bytes })
}

pub(crate) fn recover_details(address: &str, boot: BootState) -> Value {
    json!({ "address": address, "boot": boot_state(boot) })
}

pub(crate) fn restore_details(boot: BootState, safety: SafetyBackup) -> Value {
    json!({ "boot": boot_state(boot), "safety_backup": safety_backup(safety) })
}

pub(crate) fn cycle_details(servers: usize, succeeded: usize, failed: usize) -> Value {
    json!({ "servers": servers, "succeeded": succeeded, "failed": failed })
}

pub(crate) fn escalation_details(server: Option<&str>, path: Option<&str>) -> Value {
    json!({ "server": server, "path": path })
}

#[cfg(test)]
#[path = "tests/mapping.rs"]
mod tests;

pub(crate) const APPROVED: &str = "approved";

pub(crate) const DENIED: &str = "denied";

pub(crate) const SENT: &str = "sent";

pub(crate) const MAIL_UNAVAILABLE: &str = "mail_unavailable";

pub(crate) const AUTH_REFUSED: &str = "auth_refused";

pub(crate) fn mail(outcome: &crate::mail::MailOutcome) -> &'static str {
    match outcome {
        crate::mail::MailOutcome::Sent => SENT,
        crate::mail::MailOutcome::Unavailable => MAIL_UNAVAILABLE,
        crate::mail::MailOutcome::AuthRefused => AUTH_REFUSED,
        crate::mail::MailOutcome::Rejected { .. } => UNDELIVERABLE,
        crate::mail::MailOutcome::Failed { .. } => FAILED,
    }
}
