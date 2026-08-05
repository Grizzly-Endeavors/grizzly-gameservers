use super::*;

/// A quota with room to spare on every dimension, so each test can tighten the
/// one it cares about and leave the rest out of the way.
fn roomy() -> NamespaceQuota {
    NamespaceQuota {
        pvcs: Some(Dimension { hard: 32, used: 2 }),
        storage_bytes: Some(Dimension {
            hard: 600 * GIB,
            used: 20 * GIB,
        }),
        cpu_requests: Some(Dimension {
            hard: 28_000,
            used: 3_000,
        }),
        cpu_limits: Some(Dimension {
            hard: 56_000,
            used: 10_000,
        }),
        mem_requests: Some(Dimension {
            hard: 64 * GIB,
            used: 11 * GIB,
        }),
        mem_limits: Some(Dimension {
            hard: 120 * GIB,
            used: 22 * GIB,
        }),
    }
}

/// A modest server: one PVC, 5 GiB of disk, half a core requested and two cores
/// capped — roughly what the Minecraft catalog entry declares.
fn modest() -> Admission {
    Admission {
        pvcs: 1,
        storage_bytes: 5 * GIB,
        cpu_request_millicores: 500,
        cpu_limit_millicores: 2_000,
        mem_request_bytes: 2 * GIB,
        mem_limit_bytes: 4 * GIB,
    }
    .plus(SDK_SIDECAR)
}

#[test]
fn a_roomy_namespace_admits_an_ordinary_server() {
    assert_eq!(check_admission(&roomy(), modest()), Ok(()));
}

#[test]
fn an_unbounded_namespace_refuses_nothing() {
    let huge = Admission {
        pvcs: 1_000,
        storage_bytes: 900 * GIB,
        cpu_request_millicores: 900_000,
        cpu_limit_millicores: 900_000,
        mem_request_bytes: 900 * GIB,
        mem_limit_bytes: 900 * GIB,
    };
    assert_eq!(check_admission(&NamespaceQuota::default(), huge), Ok(()));
}

#[test]
fn the_pvc_ceiling_is_reported_as_a_server_ceiling() {
    let quota = NamespaceQuota {
        pvcs: Some(Dimension { hard: 16, used: 16 }),
        ..roomy()
    };
    assert_eq!(
        check_admission(&quota, modest()),
        Err(CapacityRefusal::Servers {
            current: 16,
            ceiling: 16
        })
    );
}

/// A cold start brings no PVC with it, so a namespace out of PVCs must still
/// let a stopped server come back — its claim is already counted.
#[test]
fn a_start_is_not_refused_by_the_pvc_ceiling() {
    let quota = NamespaceQuota {
        pvcs: Some(Dimension { hard: 16, used: 16 }),
        ..roomy()
    };
    let start = Admission {
        pvcs: 0,
        storage_bytes: 0,
        ..modest()
    };
    assert_eq!(check_admission(&quota, start), Ok(()));
}

#[test]
fn exhausted_cpu_requests_are_reported_before_exhausted_cpu_limits() {
    let quota = NamespaceQuota {
        cpu_requests: Some(Dimension {
            hard: 28_000,
            used: 27_900,
        }),
        cpu_limits: Some(Dimension {
            hard: 56_000,
            used: 55_900,
        }),
        ..roomy()
    };
    assert!(matches!(
        check_admission(&quota, modest()),
        Err(CapacityRefusal::Cpu {
            basis: QuotaBasis::Requests,
            ..
        })
    ));
}

#[test]
fn exhausted_memory_limits_are_reported_as_a_limits_refusal() {
    let quota = NamespaceQuota {
        mem_limits: Some(Dimension {
            hard: 120 * GIB,
            used: 119 * GIB,
        }),
        ..roomy()
    };
    assert!(matches!(
        check_admission(&quota, modest()),
        Err(CapacityRefusal::Memory {
            basis: QuotaBasis::Limits,
            ..
        })
    ));
}

/// The reserve is the whole point of #82's third bullet: game pods are created
/// continuously and the bot pod once, so without it a game server can take the
/// room the next bot pod needs and stall the rollout.
#[test]
fn the_last_slice_is_held_back_for_the_bot_s_own_rollout() {
    // Exactly one modest server's worth of memory limits free, before the
    // reserve is taken out of it.
    let free = modest().mem_limit_bytes;
    let with_used = |used| NamespaceQuota {
        mem_limits: Some(Dimension {
            hard: 120 * GIB,
            used,
        }),
        ..roomy()
    };
    assert!(matches!(
        check_admission(&with_used(120 * GIB - free), modest()),
        Err(CapacityRefusal::Memory {
            basis: QuotaBasis::Limits,
            ..
        })
    ));

    // With the reserve *also* free, the same server fits.
    let spare = 120 * GIB - free - BOT_ROLLOUT_RESERVE.mem_limit_bytes;
    assert_eq!(check_admission(&with_used(spare), modest()), Ok(()));
}

/// A quota lowered below what the namespace already holds is an operator action,
/// not a bug — the arithmetic must report no room rather than negative room.
#[test]
fn usage_beyond_the_ceiling_reports_no_room_rather_than_negative_room() {
    let quota = NamespaceQuota {
        mem_limits: Some(Dimension {
            hard: 10 * GIB,
            used: 40 * GIB,
        }),
        ..roomy()
    };
    assert_eq!(
        check_admission(&quota, modest()),
        Err(CapacityRefusal::Memory {
            basis: QuotaBasis::Limits,
            free_bytes: 0,
            adding_bytes: modest().mem_limit_bytes,
        })
    );
}

/// The sidecar is invisible in every catalog template but real to the quota.
/// A pod estimate that leaves it out passes this check and is still rejected at
/// admission, which is the failure this whole module exists to prevent.
#[test]
fn the_injected_sidecar_counts_toward_the_estimate() {
    assert!(modest().cpu_limit_millicores > 2_000);
    assert_eq!(
        modest().mem_limit_bytes,
        4 * GIB + SDK_SIDECAR.mem_limit_bytes
    );
}

#[test]
fn a_refusal_summary_names_the_dimension_and_the_figures() {
    let refusal = CapacityRefusal::Cpu {
        basis: QuotaBasis::Limits,
        free_millicores: 1_500,
        adding_millicores: 4_000,
    };
    assert_eq!(
        refusal.summary(),
        "cpu limits needs 4 cores, 1.5 cores free"
    );
    assert_eq!(
        CapacityRefusal::Servers {
            current: 16,
            ceiling: 16
        }
        .summary(),
        "server count 16/16"
    );
}
