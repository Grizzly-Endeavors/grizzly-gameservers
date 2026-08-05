use super::*;

fn policy() -> ThrottlePolicy {
    ThrottlePolicy {
        burst: 3,
        refill_interval: Duration::from_secs(10),
        max_concurrent: 2,
    }
}

fn discord(user: u64) -> TurnSubject {
    TurnSubject::Discord(user)
}

#[test]
fn refill_earns_one_token_per_interval() {
    let tokens = refilled(0.0, 3, Duration::from_secs(10), Duration::from_secs(10));
    assert!((tokens - 1.0).abs() < f64::EPSILON);
}

#[test]
fn refill_is_fractional_between_intervals() {
    let tokens = refilled(0.0, 3, Duration::from_secs(10), Duration::from_secs(5));
    assert!((tokens - 0.5).abs() < f64::EPSILON);
}

#[test]
fn refill_saturates_at_burst() {
    let tokens = refilled(0.0, 3, Duration::from_secs(10), Duration::from_secs(10_000));
    assert!((tokens - 3.0).abs() < f64::EPSILON);
}

#[test]
fn retry_after_scales_with_the_missing_fraction() {
    assert_eq!(
        retry_after(0.0, Duration::from_secs(10)),
        Duration::from_secs(10)
    );
    assert_eq!(
        retry_after(0.5, Duration::from_secs(10)),
        Duration::from_secs(5)
    );
    // Already holding a whole token: nothing to wait for.
    assert_eq!(retry_after(1.0, Duration::from_secs(10)), Duration::ZERO);
}

#[tokio::test]
async fn burst_is_spent_then_refused() {
    let throttle = TurnThrottle::new(policy());
    for _ in 0..3 {
        assert!(throttle.claim(&discord(1)).await.is_ok());
    }
    let refusal = throttle.claim(&discord(1)).await;
    assert!(matches!(
        refusal,
        Err(TurnRefusal::RateLimited { first: true, .. })
    ));
}

#[tokio::test]
async fn only_the_leading_refusal_is_flagged_for_a_reply() {
    let throttle = TurnThrottle::new(policy());
    for _ in 0..3 {
        assert!(throttle.claim(&discord(7)).await.is_ok());
    }
    let Err(TurnRefusal::RateLimited { first: leading, .. }) = throttle.claim(&discord(7)).await
    else {
        panic!("expected the first refusal");
    };
    assert!(leading, "the leading refusal should be answered");

    let Err(TurnRefusal::RateLimited {
        first: follow_on, ..
    }) = throttle.claim(&discord(7)).await
    else {
        panic!("expected a follow-on refusal");
    };
    assert!(!follow_on, "a flood must not be answered with a flood");
}

#[tokio::test]
async fn a_successful_turn_re_arms_the_reply() {
    let mut relaxed = policy();
    relaxed.burst = 1;
    let throttle = TurnThrottle::new(relaxed);
    assert!(throttle.claim(&discord(9)).await.is_ok());
    let Err(TurnRefusal::RateLimited { first: leading, .. }) = throttle.claim(&discord(9)).await
    else {
        panic!("expected a refusal once the single token is spent");
    };
    assert!(leading);
}

#[tokio::test]
async fn subjects_hold_independent_allowances() {
    let throttle = TurnThrottle::new(policy());
    for _ in 0..3 {
        assert!(throttle.claim(&discord(1)).await.is_ok());
    }
    assert!(throttle.claim(&discord(1)).await.is_err());
    // A different person is unaffected by the first one's spending.
    assert!(throttle.claim(&discord(2)).await.is_ok());
}

#[tokio::test]
async fn in_game_players_are_keyed_per_server() {
    let throttle = TurnThrottle::new(policy());
    let on_smp = TurnSubject::InGamePlayer {
        server: "smp".to_owned(),
        player: "mallory".to_owned(),
    };
    let on_other = TurnSubject::InGamePlayer {
        server: "creative".to_owned(),
        player: "mallory".to_owned(),
    };
    for _ in 0..3 {
        assert!(throttle.claim(&on_smp).await.is_ok());
    }
    assert!(throttle.claim(&on_smp).await.is_err());
    assert!(throttle.claim(&on_other).await.is_ok());
}

#[tokio::test]
async fn concurrency_permits_are_bounded_and_returned_on_drop() {
    let throttle = TurnThrottle::new(policy());
    let first = throttle.admit_now().expect("first permit");
    let second = throttle.admit_now().expect("second permit");
    assert!(matches!(throttle.admit_now(), Err(TurnRefusal::Busy)));
    drop(first);
    let third = throttle.admit_now().expect("permit freed by the drop");
    drop(second);
    drop(third);
}

#[test]
fn sweeping_drops_only_fully_refilled_buckets() {
    let now = Instant::now();
    let mut buckets = HashMap::new();
    buckets.insert(
        discord(1),
        Bucket {
            tokens: 3.0,
            updated: now,
            notified: false,
        },
    );
    buckets.insert(
        discord(2),
        Bucket {
            tokens: 0.0,
            updated: now,
            notified: true,
        },
    );
    sweep_idle(&mut buckets, policy(), now);
    assert!(
        !buckets.contains_key(&discord(1)),
        "an idle full bucket carries no state worth keeping"
    );
    assert!(
        buckets.contains_key(&discord(2)),
        "a spent bucket must survive, or the limit resets itself"
    );
}

/// Retuning has to reach the surfaces already holding this by `Arc`, which is
/// the whole reason the policy isn't a plain field.
#[test]
fn retuning_replaces_the_policy_in_place() {
    let throttle = TurnThrottle::new(ThrottlePolicy::default());
    throttle.update_policy(ThrottlePolicy {
        burst: 3,
        refill_interval: Duration::from_secs(90),
        max_concurrent: 2,
    });
    let policy = throttle.policy();
    assert_eq!(policy.burst, 3);
    assert_eq!(policy.refill_interval, Duration::from_secs(90));
    assert_eq!(policy.max_concurrent, 2);
}

#[tokio::test]
async fn raising_the_concurrency_bound_admits_more_turns_at_once() {
    let throttle = TurnThrottle::new(ThrottlePolicy {
        max_concurrent: 1,
        ..ThrottlePolicy::default()
    });
    let held = throttle.admit_now().expect("the first turn is admitted");
    assert!(throttle.admit_now().is_err(), "the bound is one");

    throttle.update_policy(ThrottlePolicy {
        max_concurrent: 3,
        ..ThrottlePolicy::default()
    });
    // The raise is immediate for new turns. The turn still holding a permit from
    // the old semaphore keeps it, which is the accepted transient overshoot.
    // Permits are bound, not dropped: an unbound `admit_now()` returns its permit
    // at the end of the statement, which would make any bound look infinite.
    let second = throttle
        .admit_now()
        .expect("the raised bound admits a second");
    let third = throttle.admit_now().expect("and a third");
    drop((held, second, third));
}

#[tokio::test]
async fn lowering_the_concurrency_bound_binds_on_the_next_turn() {
    let throttle = TurnThrottle::new(ThrottlePolicy {
        max_concurrent: 4,
        ..ThrottlePolicy::default()
    });
    let held = throttle.admit_now().expect("admitted under the old bound");
    throttle.update_policy(ThrottlePolicy {
        max_concurrent: 1,
        ..ThrottlePolicy::default()
    });
    let fresh = throttle
        .admit_now()
        .expect("the fresh semaphore has one permit");
    assert!(throttle.admit_now().is_err(), "and only one");
    drop((held, fresh));
}

/// A bucket holding more tokens than a lowered burst must not keep them: the
/// refill is capped at the burst, so it self-corrects on the next claim rather
/// than needing the map to be swept.
#[tokio::test]
async fn a_lowered_burst_caps_an_existing_allowance() {
    let throttle = TurnThrottle::new(ThrottlePolicy {
        burst: 10,
        refill_interval: Duration::from_millis(1),
        max_concurrent: 8,
    });
    let subject = TurnSubject::Discord(1);
    throttle.claim(&subject).await.expect("first claim");

    throttle.update_policy(ThrottlePolicy {
        burst: 2,
        refill_interval: Duration::from_hours(1),
        max_concurrent: 8,
    });
    // At most two claims can get through before the (now hour-long) refill binds.
    let mut admitted = 0;
    for _ in 0..6 {
        if throttle.claim(&subject).await.is_ok() {
            admitted += 1;
        }
    }
    assert!(
        admitted <= 2,
        "a lowered burst let {admitted} turns through"
    );
}

/// Retuning to the same values is a no-op, so a settings write that didn't touch
/// the throttle can't quietly reset everyone's in-flight allowance.
#[tokio::test]
async fn retuning_to_the_same_policy_leaves_permits_alone() {
    let throttle = TurnThrottle::new(ThrottlePolicy {
        max_concurrent: 1,
        ..ThrottlePolicy::default()
    });
    let held = throttle.admit_now().expect("admitted");
    throttle.update_policy(ThrottlePolicy {
        max_concurrent: 1,
        ..ThrottlePolicy::default()
    });
    assert!(
        throttle.admit_now().is_err(),
        "an unchanged policy must not install a fresh semaphore"
    );
    drop(held);
}
