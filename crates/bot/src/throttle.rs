//! Admission control for model-driven Gary turns.
//!
//! Every other bound in this system caps what a turn may *do* — the round budget
//! caps model calls within one turn, the per-instance clamp and per-guild caps
//! bound what the tools may allocate. Nothing bounded how many turns could be
//! started, and a turn is the unit that costs money: one message to Gary buys up
//! to [`DEFAULT_MAX_ROUNDS`](crate::agent::DEFAULT_MAX_ROUNDS) paid model calls.
//!
//! That gap matters because the two surfaces that start a turn are open to people
//! the operator never admitted. Anyone in a guild the bot joined can mention Gary,
//! and anyone who can *connect to a game server* can type `@Gary` in game chat —
//! a public port, no Discord identity required. The in-pod chat watcher's
//! per-player cooldown runs inside a pod whose game the attacker is a client of,
//! so it paces a polite player rather than bounding a hostile one.
//!
//! Two layers, matching the two failure shapes:
//!
//! - A **per-subject token bucket** sheds a flood at the door, before a task is
//!   spawned or a transcript is touched. Refusing is deliberate — queueing a
//!   spammer's turns just converts a burst into a backlog that still gets billed.
//! - A **global concurrency permit** bounds how much paid work runs at once,
//!   whatever mix of channels and players it arrives from.
//!
//! Sized in the spirit of [ADR-022](../../docs/decisions/022-per-guild-resource-caps.md):
//! generous enough that an honest friend iterating on a broken world never meets
//! them, tight enough that the dishonest case is bounded rather than unbounded.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tracing::{info, warn};

pub(crate) const DEFAULT_TURN_BURST: u32 = 10;
pub(crate) const DEFAULT_TURN_REFILL_SECONDS: u64 = 30;
pub(crate) const DEFAULT_MAX_CONCURRENT_TURNS: usize = 6;

const SWEEP_THRESHOLD: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum TurnSubject {
    Discord(u64),
    InGamePlayer { server: String, player: String },
}

impl TurnSubject {
    fn label(&self) -> String {
        match self {
            Self::Discord(user) => format!("discord:{user}"),
            Self::InGamePlayer { server, player } => format!("ingame:{server}:{player}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ThrottlePolicy {
    pub(crate) burst: u32,
    pub(crate) refill_interval: Duration,
    pub(crate) max_concurrent: usize,
}

impl Default for ThrottlePolicy {
    fn default() -> Self {
        Self {
            burst: DEFAULT_TURN_BURST,
            refill_interval: Duration::from_secs(DEFAULT_TURN_REFILL_SECONDS),
            max_concurrent: DEFAULT_MAX_CONCURRENT_TURNS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TurnRefusal {
    RateLimited { retry_after: Duration, first: bool },
    Busy,
}

#[derive(Clone, Copy, Debug)]
struct Bucket {
    tokens: f64,
    updated: Instant,
    notified: bool,
}

fn refilled(tokens: f64, burst: u32, refill_interval: Duration, elapsed: Duration) -> f64 {
    let interval = refill_interval.as_secs_f64();
    let earned = if interval > 0.0 {
        elapsed.as_secs_f64() / interval
    } else {
        f64::from(burst)
    };
    (tokens + earned).min(f64::from(burst))
}

fn retry_after(tokens: f64, refill_interval: Duration) -> Duration {
    let missing = (1.0 - tokens).max(0.0);
    refill_interval.mul_f64(missing)
}

pub(crate) type TurnPermit = OwnedSemaphorePermit;

pub(crate) struct TurnThrottle {
    policy: ArcSwap<ThrottlePolicy>,
    buckets: Mutex<HashMap<TurnSubject, Bucket>>,
    concurrency: ArcSwap<Semaphore>,
}

impl TurnThrottle {
    pub(crate) fn new(policy: ThrottlePolicy) -> Self {
        info!(
            burst = policy.burst,
            refill_seconds = policy.refill_interval.as_secs(),
            max_concurrent = policy.max_concurrent,
            "gary turn admission control active"
        );
        Self {
            concurrency: ArcSwap::new(Arc::new(Semaphore::new(policy.max_concurrent))),
            policy: ArcSwap::new(Arc::new(policy)),
            buckets: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn policy(&self) -> ThrottlePolicy {
        *self.policy.load_full()
    }

    pub(crate) fn update_policy(&self, policy: ThrottlePolicy) {
        let previous = self.policy();
        if previous == policy {
            return;
        }
        self.policy.store(Arc::new(policy));
        if previous.max_concurrent != policy.max_concurrent {
            self.concurrency
                .store(Arc::new(Semaphore::new(policy.max_concurrent)));
        }
        info!(
            burst = policy.burst,
            refill_seconds = policy.refill_interval.as_secs(),
            max_concurrent = policy.max_concurrent,
            "gary turn admission control retuned"
        );
    }

    pub(crate) async fn claim(&self, subject: &TurnSubject) -> Result<(), TurnRefusal> {
        let now = Instant::now();
        let policy = self.policy();
        let mut buckets = self.buckets.lock().await;
        if buckets.len() >= SWEEP_THRESHOLD {
            sweep_idle(&mut buckets, policy, now);
        }
        let bucket = buckets.entry(subject.clone()).or_insert(Bucket {
            tokens: f64::from(policy.burst),
            updated: now,
            notified: false,
        });
        bucket.tokens = refilled(
            bucket.tokens,
            policy.burst,
            policy.refill_interval,
            now.saturating_duration_since(bucket.updated),
        );
        bucket.updated = now;
        if bucket.tokens < 1.0 {
            let wait = retry_after(bucket.tokens, policy.refill_interval);
            let first = !bucket.notified;
            bucket.notified = true;
            if first {
                warn!(
                    subject = %subject.label(),
                    retry_after_seconds = wait.as_secs(),
                    "refused a gary turn: subject is rate limited"
                );
            }
            return Err(TurnRefusal::RateLimited {
                retry_after: wait,
                first,
            });
        }
        bucket.tokens -= 1.0;
        bucket.notified = false;
        Ok(())
    }

    pub(crate) fn admit_now(&self) -> Result<TurnPermit, TurnRefusal> {
        match self.concurrency.load_full().try_acquire_owned() {
            Ok(permit) => Ok(permit),
            Err(err) => {
                warn!(
                    error = ?err,
                    max_concurrent = self.policy().max_concurrent,
                    "refused a gary turn: every concurrency permit is in use"
                );
                Err(TurnRefusal::Busy)
            }
        }
    }
}

fn sweep_idle(buckets: &mut HashMap<TurnSubject, Bucket>, policy: ThrottlePolicy, now: Instant) {
    let before = buckets.len();
    buckets.retain(|_, bucket| {
        let tokens = refilled(
            bucket.tokens,
            policy.burst,
            policy.refill_interval,
            now.saturating_duration_since(bucket.updated),
        );
        tokens < f64::from(policy.burst)
    });
    info!(
        before,
        after = buckets.len(),
        "swept idle turn-throttle buckets"
    );
}

#[cfg(test)]
#[path = "tests/throttle.rs"]
mod tests;
