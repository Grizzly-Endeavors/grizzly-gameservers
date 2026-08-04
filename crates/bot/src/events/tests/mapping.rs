use super::*;

use crate::agent::EscalationReason;
use crate::events::{Actor, Surface};
use crate::notify::EscalationContext;

#[test]
fn provision_names_the_two_capacity_rejections() {
    assert_eq!(
        provision(&ProvisionOutcome::Provisioned {
            address: "host:7000".to_owned()
        }),
        "provisioned"
    );
    assert_eq!(provision(&ProvisionOutcome::AlreadyExists), "name_taken");
    assert_eq!(
        provision(&ProvisionOutcome::PortsExhausted),
        "ports_exhausted"
    );
}

#[test]
fn readiness_is_never_confused_with_an_instant_boot() {
    assert_eq!(readiness(true), "ready");
    assert_eq!(readiness(false), "unconfirmed");
}

#[test]
fn start_begin_covers_every_cold_start_result() {
    assert_eq!(
        start_begin(&StartBegin::Starting {
            address: "host:7000".to_owned()
        }),
        "starting"
    );
    assert_eq!(start_begin(&StartBegin::AlreadyRunning), "already_running");
    assert_eq!(start_begin(&StartBegin::NotFound), "not_found");
    assert_eq!(start_begin(&StartBegin::NotManaged), "not_managed");
    assert_eq!(
        start_begin(&StartBegin::UnknownGame("quake".to_owned())),
        "unknown_game"
    );
}

#[test]
fn supervisor_successes_name_the_action_that_happened() {
    assert_eq!(supervisor(&SupervisorOutcome::Paused), "stopped");
    assert_eq!(supervisor(&SupervisorOutcome::Resumed), "started");
    assert_eq!(supervisor(&SupervisorOutcome::Restarted), "restarted");
    assert_eq!(
        supervisor(&SupervisorOutcome::AlreadyStopped),
        "already_stopped"
    );
    assert_eq!(
        supervisor(&SupervisorOutcome::AlreadyRunning),
        "already_running"
    );
    assert_eq!(supervisor(&SupervisorOutcome::PodNotReady), "pod_not_ready");
    assert_eq!(supervisor(&SupervisorOutcome::Unreachable), "unreachable");
    assert_eq!(
        supervisor(&SupervisorOutcome::Failed("boom".to_owned())),
        FAILED
    );
    assert_eq!(supervisor(&SupervisorOutcome::NotFound), "not_found");
    assert_eq!(supervisor(&SupervisorOutcome::NotManaged), "not_managed");
}

#[test]
fn shutdown_and_destroy_keep_their_partial_failures_distinct() {
    assert_eq!(shutdown(&ShutdownOutcome::Down), "down");
    assert_eq!(shutdown(&ShutdownOutcome::Stranded), "stranded");
    assert_eq!(shutdown(&ShutdownOutcome::NotFound), "not_found");
    assert_eq!(shutdown(&ShutdownOutcome::NotManaged), "not_managed");

    assert_eq!(destroy(&DestroyOutcome::Destroyed), "destroyed");
    assert_eq!(destroy(&DestroyOutcome::Blocked), "blocked");
    assert_eq!(destroy(&DestroyOutcome::NotFound), "not_found");
    assert_eq!(destroy(&DestroyOutcome::NotManaged), "not_managed");
}

#[test]
fn backup_and_archive_cover_every_outcome() {
    assert_eq!(
        backup(&BackupOutcome::BackedUp { size_bytes: 1 }),
        "backed_up"
    );
    assert_eq!(backup(&BackupOutcome::NotFound), "not_found");
    assert_eq!(backup(&BackupOutcome::NotManaged), "not_managed");
    assert_eq!(backup(&BackupOutcome::NotRunning), "not_running");
    assert_eq!(
        backup(&BackupOutcome::Unreachable("gone".to_owned())),
        "unreachable"
    );

    assert_eq!(
        archive(&ArchiveOutcome::Archived {
            name: "mc".to_owned(),
            size_bytes: 1
        }),
        "archived"
    );
    assert_eq!(
        archive(&ArchiveOutcome::ArchivedNotReleased {
            name: "mc".to_owned(),
            size_bytes: 1
        }),
        "archived_not_released"
    );
    assert_eq!(archive(&ArchiveOutcome::NotFound), "not_found");
    assert_eq!(archive(&ArchiveOutcome::NotManaged), "not_managed");
    assert_eq!(archive(&ArchiveOutcome::Unavailable), "unavailable");
    assert_eq!(archive(&ArchiveOutcome::Failed("boom".to_owned())), FAILED);
}

#[test]
fn restore_and_recover_cover_every_outcome() {
    assert_eq!(
        restore(&RestoreOutcome::Restored {
            boot: BootState::Ready,
            safety_backup: SafetyBackup::Taken
        }),
        "restored"
    );
    assert_eq!(restore(&RestoreOutcome::NotFound), "not_found");
    assert_eq!(restore(&RestoreOutcome::NotManaged), "not_managed");
    assert_eq!(restore(&RestoreOutcome::Failed("boom".to_owned())), FAILED);

    assert_eq!(
        recover(&RecoverOutcome::Recovered {
            address: "host:7000".to_owned(),
            boot: BootState::Ready
        }),
        "recovered"
    );
    assert_eq!(recover(&RecoverOutcome::NoSuchArchive), "no_such_archive");
    assert_eq!(recover(&RecoverOutcome::NameInUse), "name_in_use");
    assert_eq!(recover(&RecoverOutcome::Unavailable), "unavailable");
    assert_eq!(
        recover(&RecoverOutcome::UnknownGame("quake".to_owned())),
        "unknown_game"
    );
    assert_eq!(recover(&RecoverOutcome::PortsExhausted), "ports_exhausted");
    assert_eq!(recover(&RecoverOutcome::Failed("boom".to_owned())), FAILED);
}

#[test]
fn escalation_kind_is_the_outcome() {
    let budget = Escalation::RoundBudgetExhausted {
        context: EscalationContext::Deferred {
            server: "mc".to_owned(),
            condition: "empty".to_owned(),
            guild: Some(1),
        },
        request: "fix it".to_owned(),
        attempts: vec![],
        rounds: 16,
    };
    assert_eq!(escalation(&budget), "round_budget_exhausted");
    assert_eq!(
        escalation(&Escalation::AgentRequested {
            context: EscalationContext::Deferred {
                server: "mc".to_owned(),
                condition: "empty".to_owned(),
                guild: Some(1),
            },
            request: "fix it".to_owned(),
            reason: "needs a human".to_owned(),
            server: None,
            attempts: vec![],
        }),
        "agent_requested"
    );
    assert_eq!(
        escalation(&Escalation::CrashRollback {
            server: "mc".to_owned(),
            path: "server.properties".to_owned()
        }),
        "crash_rollback"
    );
}

/// A Discord handoff carries no server in its context, so the one the model
/// named in the tool call is the only locator the row can have — without this,
/// the `instance` column is null for every agent-requested escalation.
#[test]
fn an_agent_requested_escalation_records_the_server_the_model_named() {
    let event = escalation_event(
        &Escalation::AgentRequested {
            context: EscalationContext::Discord {
                asker: "Alice".to_owned(),
                jump_link: "https://discord.com/channels/1/2/3".to_owned(),
                guild: Some(1),
            },
            request: "fix minecraft".to_owned(),
            reason: "needs a human".to_owned(),
            server: Some("mc-summer".to_owned()),
            attempts: vec![],
        },
        &Attribution::new(Actor::Discord(42), Surface::Agent),
        None,
    );
    assert_eq!(
        event.instance.as_ref().map(InstanceName::as_str),
        Some("mc-summer")
    );
    assert_eq!(
        event.details,
        escalation_details(Some("mc-summer"), None),
        "the details payload names it too, matching every other escalation shape"
    );
}

/// The loop names its exits and the event log names its outcomes; they have to
/// be the same names or the Langfuse `escalation_kind` score can't be joined to
/// an `events` row. `agent::session` can't reach these constants (it's the pure
/// core), so the agreement is asserted here instead.
#[test]
fn the_loops_escalation_names_match_the_event_outcomes() {
    assert_eq!(
        EscalationReason::RoundBudget.as_str(),
        ROUND_BUDGET_EXHAUSTED
    );
    assert_eq!(
        EscalationReason::AgentRequested {
            reason: String::new(),
            server: None,
        }
        .as_str(),
        AGENT_REQUESTED
    );
}

#[test]
fn an_unconfirmed_boot_reports_unknown_timing_not_zero() {
    assert_eq!(
        boot_details(Some("host:7000"), None),
        serde_json::json!({ "address": "host:7000", "time_to_ready_ms": null })
    );
    assert_eq!(
        boot_details(None, Some(Duration::from_millis(4200))),
        serde_json::json!({ "address": null, "time_to_ready_ms": 4200 })
    );
}

#[test]
fn payload_builders_produce_the_documented_shapes() {
    assert_eq!(size_details(99), serde_json::json!({ "size_bytes": 99 }));
    assert_eq!(
        recover_details("host:7000", BootState::Crashed),
        serde_json::json!({ "address": "host:7000", "boot": "crashed" })
    );
    assert_eq!(
        restore_details(BootState::TimedOut, SafetyBackup::Absent),
        serde_json::json!({ "boot": "timed_out", "safety_backup": "absent" })
    );
    assert_eq!(
        cycle_details(5, 4, 1),
        serde_json::json!({ "servers": 5, "succeeded": 4, "failed": 1 })
    );
    assert_eq!(
        escalation_details(Some("mc"), None),
        serde_json::json!({ "server": "mc", "path": null })
    );
}

#[test]
fn boot_and_safety_states_are_named_not_numbered() {
    assert_eq!(boot_state(BootState::Ready), "ready");
    assert_eq!(boot_state(BootState::Crashed), "crashed");
    assert_eq!(boot_state(BootState::Stopped), "stopped");
    assert_eq!(boot_state(BootState::TimedOut), "timed_out");
    assert_eq!(safety_backup(SafetyBackup::Taken), "taken");
    assert_eq!(safety_backup(SafetyBackup::Absent), "absent");
}
