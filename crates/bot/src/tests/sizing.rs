use super::*;

/// A sizing comfortably inside every bound, to perturb one figure at a time.
fn valid() -> Sizing {
    Sizing {
        cpu_request_millicores: 500,
        mem_request_bytes: 2 * 1024 * 1024 * 1024,
        cpu_limit_millicores: 2000,
        mem_limit_bytes: 4 * 1024 * 1024 * 1024,
    }
}

#[test]
fn a_sizing_within_every_bound_is_accepted() {
    assert_eq!(valid().validate(), Ok(()));
}

#[test]
fn the_floors_are_inclusive() {
    // Exactly the floor is viable; the refusal is for going below it.
    let floor = Sizing {
        cpu_request_millicores: MIN_CPU_MILLICORES,
        mem_request_bytes: MIN_MEM_BYTES,
        ..valid()
    };
    assert_eq!(floor.validate(), Ok(()));

    assert_eq!(
        Sizing {
            cpu_request_millicores: MIN_CPU_MILLICORES - 1,
            ..floor
        }
        .validate(),
        Err(SizingRejection::CpuBelowFloor)
    );
    assert_eq!(
        Sizing {
            mem_request_bytes: MIN_MEM_BYTES - 1,
            ..floor
        }
        .validate(),
        Err(SizingRejection::MemoryBelowFloor)
    );
}

#[test]
fn the_ceilings_are_inclusive() {
    let ceiling = Sizing {
        cpu_limit_millicores: MAX_CPU_MILLICORES,
        mem_limit_bytes: MAX_MEM_BYTES,
        ..valid()
    };
    assert_eq!(ceiling.validate(), Ok(()));

    assert_eq!(
        Sizing {
            cpu_limit_millicores: MAX_CPU_MILLICORES + 1,
            ..ceiling
        }
        .validate(),
        Err(SizingRejection::CpuAboveCeiling)
    );
    assert_eq!(
        Sizing {
            mem_limit_bytes: MAX_MEM_BYTES + 1,
            ..ceiling
        }
        .validate(),
        Err(SizingRejection::MemoryAboveCeiling)
    );
}

#[test]
fn a_request_above_its_limit_is_refused_rather_than_left_unschedulable() {
    // Kubernetes rejects the pod outright for this, so catching it here turns a
    // server that never starts into a message naming the mistake.
    assert_eq!(
        Sizing {
            cpu_request_millicores: 3000,
            cpu_limit_millicores: 2000,
            ..valid()
        }
        .validate(),
        Err(SizingRejection::CpuRequestExceedsLimit)
    );
    assert_eq!(
        Sizing {
            mem_request_bytes: 8 * 1024 * 1024 * 1024,
            mem_limit_bytes: 4 * 1024 * 1024 * 1024,
            ..valid()
        }
        .validate(),
        Err(SizingRejection::MemoryRequestExceedsLimit)
    );
}

#[test]
fn a_request_equal_to_its_limit_is_fine() {
    // Guaranteed QoS — a legitimate thing to ask for, not an off-by-one.
    assert_eq!(
        Sizing {
            cpu_request_millicores: 2000,
            cpu_limit_millicores: 2000,
            mem_request_bytes: 4 * 1024 * 1024 * 1024,
            mem_limit_bytes: 4 * 1024 * 1024 * 1024,
        }
        .validate(),
        Ok(())
    );
}

#[test]
fn quantities_render_in_units_kubernetes_parses_exactly() {
    let rendered = valid().as_quantities();
    assert_eq!(rendered.cpu_request, "500m");
    assert_eq!(rendered.cpu_limit, "2000m");
    // Plain bytes rather than Gi: an exact integer can't round on the way in.
    assert_eq!(rendered.mem_request, "2147483648");
    assert_eq!(rendered.mem_limit, "4294967296");
}

#[tokio::test]
async fn a_disabled_facade_reports_itself_and_holds_nothing() {
    // No DB_PASSWORD: servers keep running on catalog sizing, and the tool has
    // to refuse rather than apply a change that would vanish on next start.
    let sizing = InstanceSizing::connect(None).await;
    assert!(!sizing.is_available());
    assert_eq!(sizing.get("mc-summer").await, None);
}

#[tokio::test]
async fn setting_a_sizing_without_a_database_fails_rather_than_pretending() {
    let sizing = InstanceSizing::connect(None).await;
    let result = sizing
        .set(&InstanceName::new("mc-summer"), valid(), "42")
        .await;
    assert!(
        result.is_err(),
        "a resize that can't be recorded must not report success"
    );
}

#[tokio::test]
async fn forgetting_without_a_database_is_a_no_op_not_an_error() {
    // Destroy calls this on every teardown; a disabled façade has nothing stale
    // to clear, so failing here would turn a clean destroy into a reported error.
    let sizing = InstanceSizing::connect(None).await;
    assert!(sizing.forget("mc-summer").await.is_ok());
}
