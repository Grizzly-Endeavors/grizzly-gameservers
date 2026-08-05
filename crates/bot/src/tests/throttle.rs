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
