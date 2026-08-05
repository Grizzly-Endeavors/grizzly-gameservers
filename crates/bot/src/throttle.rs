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

use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tracing::{info, warn};

/// Turns a subject may start back-to-back before the refill rate binds.
pub(crate) const DEFAULT_TURN_BURST: u32 = 10;
/// Seconds to earn one turn back. At the default this is 120 turns/hour
/// sustained per subject after the burst is spent.
pub(crate) const DEFAULT_TURN_REFILL_SECONDS: u64 = 30;
/// Model-driven turns allowed to run at once across every surface.
pub(crate) const DEFAULT_MAX_CONCURRENT_TURNS: usize = 6;

/// Subjects tracked before the map is swept of idle entries. In-game player
/// names are attacker-chosen, so the map must not grow with them unboundedly.
const SWEEP_THRESHOLD: usize = 512;

/// Who a turn is billed to. In-game chat carries no Discord identity, so a player
/// is keyed by the server they're on plus the name they chose — the same pair the
/// reply is routed back to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum TurnSubject {
    Discord(u64),
    InGamePlayer { server: String, player: String },
}

impl TurnSubject {
    /// A short label for logs. Deliberately not the raw player string in the
    /// Discord case — a user id is already the stable identifier.
    fn label(&self) -> String {
        match self {
            Self::Discord(user) => format!("discord:{user}"),
            Self::InGamePlayer { server, player } => format!("ingame:{server}:{player}"),
        }
    }
}

/// How many turns a subject may start, and how fast the allowance returns.
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

/// Why a turn was not admitted. Each maps to a distinct thing to tell the person,
/// because "slow down" and "I'm busy" call for different follow-up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TurnRefusal {
    /// The subject spent their allowance; it returns at the refill rate.
    /// `first` marks the leading edge of a run of refusals — the only one worth
    /// answering out loud, since replying to every refused message would answer
    /// a flood with a flood.
    RateLimited { retry_after: Duration, first: bool },
    /// Every concurrency permit is in use by other people's turns.
    Busy,
}

/// One subject's allowance. `tokens` is fractional so a partial refill isn't lost
/// to truncation between closely-spaced messages.
#[derive(Clone, Copy, Debug)]
struct Bucket {
    tokens: f64,
    updated: Instant,
    /// Whether the subject has already been told they're rate-limited. Replying
    /// to every refused message would answer a flood with a flood.
    notified: bool,
}

/// Tokens a bucket holds after `elapsed`, capped at `burst`. Pure so the refill
/// curve is tested without sleeping.
fn refilled(tokens: f64, burst: u32, refill_interval: Duration, elapsed: Duration) -> f64 {
    let interval = refill_interval.as_secs_f64();
    let earned = if interval > 0.0 {
        elapsed.as_secs_f64() / interval
    } else {
        f64::from(burst)
    };
    (tokens + earned).min(f64::from(burst))
}

/// How long until a bucket holds a whole token again.
fn retry_after(tokens: f64, refill_interval: Duration) -> Duration {
    let missing = (1.0 - tokens).max(0.0);
    refill_interval.mul_f64(missing)
}

/// Held for the life of an admitted turn; dropping it returns the permit.
pub(crate) type TurnPermit = OwnedSemaphorePermit;

/// The admission gate shared by every surface that can start a Gary turn.
pub(crate) struct TurnThrottle {
    policy: ThrottlePolicy,
    buckets: Mutex<HashMap<TurnSubject, Bucket>>,
    concurrency: Arc<Semaphore>,
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
            concurrency: Arc::new(Semaphore::new(policy.max_concurrent)),
            policy,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// The policy in force right now.
    pub(crate) fn policy(&self) -> ThrottlePolicy {
        self.policy
    }

    /// Spend one of `subject`'s turns, or refuse.
    ///
    /// Call this at the door — before spawning a task or touching a transcript —
    /// so a flood costs a map lookup rather than a model call. A refusal carries
    /// `first`, marking the leading edge of a run so the caller can answer once
    /// and then stay quiet.
    pub(crate) async fn claim(&self, subject: &TurnSubject) -> Result<(), TurnRefusal> {
        let now = Instant::now();
        // Copied once at the door so every decision in this call is made against
        // one policy, even if the operator retunes it mid-claim.
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
        // A turn got through, so the next refusal starts a fresh run and is worth
        // answering again.
        bucket.notified = false;
        Ok(())
    }

    /// Take a concurrency permit for a turn that is about to run, or report that
    /// the system is saturated. Acquire this *after* any per-channel serialization
    /// so a permit is only held by work that is actually running.
    pub(crate) fn admit_now(&self) -> Result<TurnPermit, TurnRefusal> {
        match Arc::clone(&self.concurrency).try_acquire_owned() {
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

/// Drop buckets that have refilled completely — they carry no state a fresh
/// entry wouldn't reproduce, so forgetting them is free and bounds the map
/// against attacker-chosen player names.
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
