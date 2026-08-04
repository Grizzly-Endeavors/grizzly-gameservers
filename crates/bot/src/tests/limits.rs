use super::*;
use crate::config::GuildLimitDefaults;

const DEFAULTS: GuildLimitDefaults = GuildLimitDefaults {
    servers: 5,
    cpu_limit_millicores: 24_000,
    mem_limit_bytes: 48 * 1024 * 1024 * 1024,
};

fn sizing(cpu_limit: i64, mem_limit: i64) -> Sizing {
    Sizing {
        cpu_request_millicores: cpu_limit / 2,
        mem_request_bytes: mem_limit / 2,
        cpu_limit_millicores: cpu_limit,
        mem_limit_bytes: mem_limit,
    }
}

fn usage(count: usize, cpu: i64, mem: i64) -> GuildUsage {
    GuildUsage {
        server_count: count,
        live_cpu_limit_millicores: cpu,
        live_mem_limit_bytes: mem,
    }
}

#[test]
fn effective_caps_all_defaults_without_overrides() {
    let caps = effective_caps(None, DEFAULTS);
    assert_eq!(caps.servers, 5);
    assert_eq!(caps.cpu_limit_millicores, 24_000);
    assert_eq!(caps.mem_limit_bytes, 48 * 1024 * 1024 * 1024);
}

#[test]
fn effective_caps_merges_each_column_independently() {
    let overrides = GuildLimitOverrides {
        servers: Some(8),
        cpu_limit_millicores: None,
        mem_limit_bytes: Some(64 * 1024 * 1024 * 1024),
    };
    let caps = effective_caps(Some(overrides), DEFAULTS);
    assert_eq!(caps.servers, 8);
    assert_eq!(
        caps.cpu_limit_millicores, 24_000,
        "unset column falls to default"
    );
    assert_eq!(caps.mem_limit_bytes, 64 * 1024 * 1024 * 1024);
}

#[test]
fn create_admitted_below_count_cap() {
    let caps = effective_caps(None, DEFAULTS);
    let result = check_create(&usage(4, 0, 0), Some(sizing(2_000, 4 << 30)), &caps);
    assert_eq!(result, Ok(()));
}

#[test]
fn create_refused_at_count_cap() {
    let caps = effective_caps(None, DEFAULTS);
    let result = check_create(&usage(5, 0, 0), None, &caps);
    assert_eq!(
        result,
        Err(LimitRefusal::ServerCount { current: 5, cap: 5 })
    );
}

#[test]
fn create_count_checked_before_aggregate() {
    // Both caps violated: the count refusal wins, since it's the one the friend
    // can act on without resource arithmetic.
    let caps = effective_caps(None, DEFAULTS);
    let result = check_create(&usage(5, 24_000, 0), Some(sizing(2_000, 1 << 30)), &caps);
    assert!(matches!(result, Err(LimitRefusal::ServerCount { .. })));
}

#[test]
fn create_aggregate_exactly_at_cap_admitted() {
    let caps = effective_caps(None, DEFAULTS);
    let result = check_create(
        &usage(1, 16_000, 32 << 30),
        Some(sizing(8_000, 16 << 30)),
        &caps,
    );
    assert_eq!(result, Ok(()));
}

#[test]
fn create_aggregate_over_cap_refused_with_figures() {
    let caps = effective_caps(None, DEFAULTS);
    let result = check_create(&usage(1, 16_001, 0), Some(sizing(8_000, 16 << 30)), &caps);
    assert_eq!(
        result,
        Err(LimitRefusal::AggregateCpu {
            current_millicores: 16_001,
            adding_millicores: 8_000,
            cap_millicores: 24_000,
        })
    );
}

#[test]
fn create_memory_over_cap_refused_with_figures() {
    let caps = effective_caps(None, DEFAULTS);
    let result = check_create(&usage(1, 0, 33 << 30), Some(sizing(1_000, 16 << 30)), &caps);
    assert_eq!(
        result,
        Err(LimitRefusal::AggregateMemory {
            current_bytes: 33 << 30,
            adding_bytes: 16 << 30,
            cap_bytes: 48 << 30,
        })
    );
}

#[test]
fn create_with_unreadable_catalog_sizing_contributes_zero() {
    // A template whose figures couldn't be parsed must not turn into a
    // guild-wide create outage; the count cap still applies.
    let caps = effective_caps(None, DEFAULTS);
    assert_eq!(
        check_create(&usage(2, 24_000, 48 << 30), None, &caps),
        Ok(())
    );
}

#[test]
fn added_load_admitted_when_it_fits() {
    let caps = effective_caps(None, DEFAULTS);
    let result = check_added_load(&usage(3, 8_000, 16 << 30), sizing(16_000, 32 << 30), &caps);
    assert_eq!(result, Ok(()));
}

#[test]
fn added_load_refused_when_it_overflows() {
    let caps = effective_caps(None, DEFAULTS);
    let result = check_added_load(&usage(3, 8_001, 0), sizing(16_000, 1 << 30), &caps);
    assert!(matches!(result, Err(LimitRefusal::AggregateCpu { .. })));
}

#[test]
fn resize_semantics_exclude_target_contribution() {
    // A server at 8c being resized to 16c in a guild whose *other* servers hold
    // 8c: the caller measures usage with the target excluded, so the proposal
    // replaces the old figure instead of stacking onto it.
    let caps = effective_caps(None, DEFAULTS);
    let others = usage(2, 8_000, 16 << 30);
    assert_eq!(
        check_added_load(&others, sizing(16_000, 32 << 30), &caps),
        Ok(())
    );
}

#[test]
fn aggregate_arithmetic_saturates() {
    let caps = effective_caps(None, DEFAULTS);
    let result = check_added_load(&usage(1, i64::MAX, 0), sizing(1_000, 1 << 30), &caps);
    assert!(matches!(result, Err(LimitRefusal::AggregateCpu { .. })));
}

#[test]
fn oversized_server_count_still_refuses() {
    let caps = effective_caps(None, DEFAULTS);
    let result = check_create(&usage(usize::MAX, 0, 0), None, &caps);
    assert!(matches!(result, Err(LimitRefusal::ServerCount { .. })));
}
