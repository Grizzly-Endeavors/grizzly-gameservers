//! Pure decision logic for the update-on-empty auto-updater.
//!
//! Every game image in this project (re-)pulls the latest server build when its
//! entrypoint runs, so a plain in-place relaunch *is* an update. This module
//! decides *when* to spend that relaunch: it watches occupancy across polls and
//! bounces an idle server once it's been empty long enough and its current build
//! is old enough — zero disruption, because nobody's connected. A hard version-age
//! cap is the backstop so a server that never empties still can't drift forever.
//!
//! "Version age" is just the current child's uptime: every relaunch restarts the
//! child and re-pulls, so time-on-current-build resets naturally without a
//! separate timer. All timing is injected (`now`, `version_age`) so the logic is
//! deterministic under test — the runner is the only place that reads the clock.

use std::time::{Duration, Instant};

/// Thresholds governing when an idle server is bounced to pick up a game update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutoUpdatePolicy {
    /// How long a server must stay continuously empty before an idle
    /// update-relaunch fires — long enough that a brief gap between sessions
    /// doesn't bounce the server out from under someone about to reconnect.
    pub empty_grace: Duration,
    /// Minimum version-age (child uptime) before an idle server is worth
    /// bouncing. Keeps a freshly (re)launched server — already on the latest
    /// build — from immediately relaunching again the moment it's empty.
    pub update_interval: Duration,
    /// Hard cap on version-age: once the current build is this old, relaunch even
    /// if players are online (after a heads-up broadcast) so a continuously-busy
    /// server can't fall arbitrarily far behind the current build.
    pub max_uptime: Duration,
}

/// Why the auto-updater decided to relaunch, which the runner uses to pick how
/// disruptive the bounce is allowed to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelaunchReason {
    /// The server is empty and its build is past [`AutoUpdatePolicy::update_interval`]
    /// — a silent bounce nobody is around to notice.
    Idle,
    /// Version-age hit [`AutoUpdatePolicy::max_uptime`] while players are still
    /// online — warn them, then bounce, rather than let the build keep drifting.
    Backstop,
}

/// Tracks occupancy across polls and decides when a relaunch is warranted.
#[derive(Debug)]
pub struct AutoUpdater {
    policy: AutoUpdatePolicy,
    /// When the server was first observed empty in the current empty streak, or
    /// `None` while it's occupied (or occupancy is unknown and no streak is open).
    empty_since: Option<Instant>,
}

impl AutoUpdater {
    #[must_use]
    pub fn new(policy: AutoUpdatePolicy) -> Self {
        Self {
            policy,
            empty_since: None,
        }
    }

    /// Fold one occupancy reading into the tracker and decide whether to relaunch.
    ///
    /// `players` is `None` when occupancy couldn't be determined (a stopped or
    /// still-starting console, or an unparseable reply). Unknown is treated as
    /// "don't act" and leaves any open empty streak intact, so a single transient
    /// hiccup between two empty polls neither triggers a bounce nor resets the
    /// grace clock. `version_age` is the current child's uptime.
    pub fn observe(
        &mut self,
        players: Option<u32>,
        version_age: Duration,
        now: Instant,
    ) -> Option<RelaunchReason> {
        match players {
            None => None,
            Some(0) => {
                let since = *self.empty_since.get_or_insert(now);
                let empty_for = now.saturating_duration_since(since);
                (empty_for >= self.policy.empty_grace && version_age >= self.policy.update_interval)
                    .then_some(RelaunchReason::Idle)
            }
            Some(_) => {
                self.empty_since = None;
                (version_age >= self.policy.max_uptime).then_some(RelaunchReason::Backstop)
            }
        }
    }

    /// Reset the empty streak after a relaunch. The version-age gate already
    /// blocks an immediate re-trigger (the new child's uptime is ~0), but clearing
    /// the streak restarts the grace clock cleanly against the fresh child.
    pub fn note_relaunched(&mut self) {
        self.empty_since = None;
    }
}

#[cfg(test)]
#[path = "tests/autoupdate.rs"]
mod tests;
