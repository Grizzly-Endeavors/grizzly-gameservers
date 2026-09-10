//! Stopping a Gary turn that is already running.
//!
//! A turn is the unit that costs money: one message buys up to
//! [`gary_max_rounds`](crate::settings) paid model calls, and a turn that has
//! stopped converging spends the rest of that budget before anyone can do
//! anything about it. The round budget bounds the damage; this is how a person
//! ends it sooner.
//!
//! Scoped to the channel, because that is what a conversation is
//! ([ADR-020](../../docs/decisions/020-channel-bound-conversations.md)) and
//! because the session store already serialises turns per channel — so there is
//! exactly one turn to stop and no ambiguity about which. It is deliberately
//! ungated for the same reason `/new-session` is: the transcript is shared, so
//! anyone who can add to it can end a turn against it, and the cost of a stop
//! nobody wanted is that somebody asks again.
//!
//! What a stop does *not* do is interrupt a tool. Cancellation is observed by
//! the agent loop between rounds and against the model call it is waiting on;
//! a `restart_server` already in flight runs to completion, because a config
//! write with its verification abandoned half-way is worse than a turn that
//! takes a few more seconds to stop.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio_util::sync::CancellationToken;

/// One registration, identified so a guard can tell its own entry from the one
/// a later turn put in the same slot.
#[derive(Debug)]
struct Registration {
    id: u64,
    token: CancellationToken,
}

/// The turns currently running, by the channel each belongs to.
///
/// A plain `Mutex` rather than an async one: every critical section here is a
/// map lookup with no await inside it, so the lock is never held across a yield.
#[derive(Debug, Default)]
pub(crate) struct TurnInterrupts {
    running: Mutex<HashMap<u64, Registration>>,
    /// Source of the registration ids. Only ever compared for equality, so
    /// wrapping after 2^64 turns is not a case worth handling.
    next_id: AtomicU64,
}

impl TurnInterrupts {
    /// Register a turn starting in `channel` and hand back its cancellation
    /// token plus the guard that deregisters it.
    ///
    /// The guard is what keeps the map from growing: it clears the entry on
    /// drop, so a turn that ends by any route — answered, escalated, stopped, or
    /// unwound by a panic — takes its registration with it.
    ///
    /// A second turn registering for a channel replaces the first's entry.
    /// Turns are serialised per channel upstream, so that only happens in the
    /// window where one is being torn down as the next starts; the displaced
    /// guard finding a different token in the slot is what stops it removing
    /// the newcomer's.
    pub(crate) fn register(&self, channel: u64) -> (CancellationToken, InterruptGuard<'_>) {
        let token = CancellationToken::new();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut running) = self.running.lock() {
            running.insert(
                channel,
                Registration {
                    id,
                    token: token.clone(),
                },
            );
        }
        (
            token,
            InterruptGuard {
                interrupts: self,
                channel,
                id,
            },
        )
    }

    /// Stop the turn running in `channel`. Returns whether there was one.
    ///
    /// Idempotent: cancelling an already-cancelled turn reports `true` and
    /// changes nothing, which is the honest answer — a turn is on its way out
    /// either way.
    pub(crate) fn cancel(&self, channel: u64) -> bool {
        let Ok(running) = self.running.lock() else {
            return false;
        };
        let Some(registration) = running.get(&channel) else {
            return false;
        };
        registration.token.cancel();
        true
    }

    /// Whether a turn is currently registered for `channel`.
    ///
    /// Only the tests ask: the command path reads the answer out of
    /// [`Self::cancel`]'s return, so a separate check on the live path would be
    /// a second lock acquisition racing the first.
    #[cfg(test)]
    pub(crate) fn is_running(&self, channel: u64) -> bool {
        self.running
            .lock()
            .is_ok_and(|running| running.contains_key(&channel))
    }
}

/// Deregisters a turn when it ends. See [`TurnInterrupts::register`].
#[derive(Debug)]
pub(crate) struct InterruptGuard<'a> {
    interrupts: &'a TurnInterrupts,
    channel: u64,
    id: u64,
}

impl Drop for InterruptGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut running) = self.interrupts.running.lock() {
            // Only remove our own registration. A later turn for the same
            // channel owns the slot now, and taking it would leave that turn
            // unstoppable.
            if running
                .get(&self.channel)
                .is_some_and(|registration| registration.id == self.id)
            {
                running.remove(&self.channel);
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/interrupt.rs"]
mod tests;
