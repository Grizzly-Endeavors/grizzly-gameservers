use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutoUpdatePolicy {
    pub empty_grace: Duration,
    pub update_interval: Duration,
    pub max_uptime: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelaunchReason {
    Idle,
    Backstop,
}

#[derive(Debug)]
pub struct AutoUpdater {
    policy: AutoUpdatePolicy,
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

    pub fn note_relaunched(&mut self) {
        self.empty_since = None;
    }
}

#[cfg(test)]
#[path = "tests/autoupdate.rs"]
mod tests;
