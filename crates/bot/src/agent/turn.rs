//! The identity of one Gary turn, independent of how that turn was delivered.
//!
//! Every entry point — an `@mention`, an in-game `@Gary`, a deferred batch, the
//! operator harness — fills this in before the loop runs, so "which surface is
//! this?" is a value the turn carries rather than something downstream code
//! reconstructs from circumstantial evidence. A new surface has to answer these
//! questions to run a turn at all.

use super::store::SessionId;
use crate::discord::AccessLevel;

/// Which entry point started this turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TurnOrigin {
    /// A Discord message — an `@mention`, a DM, or a home channel.
    Discord,
    /// A player's `@Gary` in game chat, arriving over the supervisor's trigger.
    Ingame,
    /// A `run_when` batch fired by its condition, with no one waiting on it.
    Deferred,
    /// The operator harness driving the deployed bot over HTTP.
    Operator,
    /// The proactive watchdog responding to a detected incident — nobody asked.
    Watchdog,
}

impl TurnOrigin {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Discord => "discord",
            Self::Ingame => "ingame",
            Self::Deferred => "deferred",
            Self::Operator => "operator",
            Self::Watchdog => "watchdog",
        }
    }
}

/// Which population a turn's telemetry belongs to. Kept separate from
/// [`TurnOrigin`] because it answers a different question: origin says how the
/// turn arrived, environment says whether it counts as real traffic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Environment {
    /// Real friends using the deployed bot.
    Production,
    /// Operator-harness traffic, quarantined so test turns can never be mistaken
    /// for product signal.
    Operator,
    /// An off-cluster developer run.
    Development,
}

impl Environment {
    /// Narrow a deployment's baseline environment for one turn. Operator-harness
    /// traffic is quarantined wherever it runs; every other origin inherits the
    /// deployment's own.
    fn for_origin(deployment: Self, origin: TurnOrigin) -> Self {
        match origin {
            TurnOrigin::Operator => Self::Operator,
            // Watchdog turns are real production traffic: they act on real
            // servers with no harness in the loop.
            TurnOrigin::Discord
            | TurnOrigin::Ingame
            | TurnOrigin::Deferred
            | TurnOrigin::Watchdog => deployment,
        }
    }

    /// The Langfuse environment name. Dashboards filter on these, so they are a
    /// published vocabulary rather than a rendering detail.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Production => "production",
            Self::Operator => "operator",
            Self::Development => "development",
        }
    }
}

/// Who asked. A Discord snowflake for every surface that has one; in-game
/// players have no Discord identity, only the name they play under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TurnActor {
    Discord(u64),
    Player(String),
}

/// Everything a turn knows about itself before the model is asked anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TurnIdentity {
    pub(crate) origin: TurnOrigin,
    /// The environment this process reports for ordinary traffic. Read through
    /// [`TurnIdentity::environment`], which narrows it by origin.
    pub(crate) deployment: Environment,
    pub(crate) guild: Option<u64>,
    /// Where the reply lands. `None` in game, where the answer is broadcast over
    /// RCON rather than posted to a channel.
    pub(crate) channel: Option<u64>,
    pub(crate) actor: TurnActor,
    pub(crate) access: AccessLevel,
    /// The conversation instance this turn continues, when there is one. `None`
    /// for a deferred batch, which is a one-shot conversation with no history.
    pub(crate) session: Option<SessionId>,
}

impl TurnIdentity {
    /// The environment this turn's telemetry belongs to.
    pub(crate) fn environment(&self) -> Environment {
        Environment::for_origin(self.deployment, self.origin)
    }
}

#[cfg(test)]
#[path = "tests/turn.rs"]
mod tests;
