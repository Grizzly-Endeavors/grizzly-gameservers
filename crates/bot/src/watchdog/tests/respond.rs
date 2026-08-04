use super::*;
use crate::domain::{GameId, GuildId, InstanceName};

fn incident(facts: IncidentFacts, kind: IncidentKind) -> Incident {
    Incident {
        instance: InstanceName::new("mc-a"),
        game: Some(GameId::new("minecraft")),
        guild: Some(GuildId::new("42")),
        kind,
        facts,
    }
}

#[test]
fn storm_guard_counts_only_the_window() {
    let t0 = Instant::now();
    let mut starts: VecDeque<Instant> = VecDeque::new();
    assert!(
        !storm_suppressed(&mut starts, t0),
        "an empty window is calm"
    );

    starts.push_back(t0);
    starts.push_back(t0 + Duration::from_mins(5));
    assert!(
        !storm_suppressed(&mut starts, t0 + Duration::from_mins(10)),
        "two recent turns are under the cap"
    );

    starts.push_back(t0 + Duration::from_mins(10));
    assert!(
        storm_suppressed(&mut starts, t0 + Duration::from_mins(11)),
        "a third turn inside the window trips the guard"
    );

    // The first two starts age out of the window; the storm has passed.
    assert!(!storm_suppressed(&mut starts, t0 + Duration::from_mins(36)));
    assert_eq!(starts.len(), 1, "aged-out starts must be pruned");
}

#[test]
fn incident_prompt_carries_the_server_and_the_crash_loop_numbers() {
    let rendered = compose_incident_prompt(&incident(
        IncidentFacts::CrashLoop {
            restarts_delta: 2,
            window_minutes: 15,
            total_restarts: 7,
            uptime_seconds: 30,
        },
        IncidentKind::Crash,
    ));
    assert!(
        rendered.contains("\"mc-a\""),
        "server name must be quoted in: {rendered}"
    );
    assert!(
        rendered.contains('2'),
        "restart delta must land in: {rendered}"
    );
    assert!(
        rendered.contains("15"),
        "window minutes must land in: {rendered}"
    );
}

#[test]
fn incident_prompt_renders_each_remaining_kind() {
    let crashed = compose_incident_prompt(&incident(
        IncidentFacts::Crashed { total_restarts: 5 },
        IncidentKind::Crash,
    ));
    assert!(
        crashed.contains('5'),
        "restart total must land in: {crashed}"
    );

    let wedged = compose_incident_prompt(&incident(
        IncidentFacts::Unreachable { minutes: 15 },
        IncidentKind::Wedged,
    ));
    assert!(wedged.contains("15"), "minutes must land in: {wedged}");

    let memory = compose_incident_prompt(&incident(
        IncidentFacts::MemoryPressure {
            used_bytes: 950 * (1 << 20),
            limit_bytes: 1000 * (1 << 20),
            percent: 95,
        },
        IncidentKind::Memory,
    ));
    assert!(memory.contains("950"), "used MiB must land in: {memory}");
    assert!(memory.contains("1000"), "limit MiB must land in: {memory}");
    assert!(memory.contains("95"), "percent must land in: {memory}");

    let unhealthy =
        compose_incident_prompt(&incident(IncidentFacts::Unhealthy, IncidentKind::Crash));
    assert!(
        unhealthy.contains("\"mc-a\""),
        "the frame must still name the server: {unhealthy}"
    );
}

#[test]
fn facts_lines_render_human_readable_numbers() {
    let memory_line = facts_line(&IncidentFacts::MemoryPressure {
        used_bytes: 3 * (1 << 30),
        limit_bytes: 4 * (1 << 30),
        percent: 75,
    });
    assert_eq!(memory_line, "memory at 3072 MiB of 4096 MiB (75%)");

    let wedged_line = facts_line(&IncidentFacts::Unreachable { minutes: 15 });
    assert!(wedged_line.contains("15 minutes"));

    let crash_loop_line = facts_line(&IncidentFacts::CrashLoop {
        restarts_delta: 2,
        window_minutes: 15,
        total_restarts: 9,
        uptime_seconds: 12,
    });
    assert!(
        crash_loop_line.contains('2')
            && crash_loop_line.contains("15")
            && crash_loop_line.contains('9')
    );

    let crashed_line = facts_line(&IncidentFacts::Crashed { total_restarts: 6 });
    assert!(crashed_line.contains('6') && crashed_line.contains("down"));
}
