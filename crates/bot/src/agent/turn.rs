use super::store::SessionId;
use crate::discord::AccessLevel;
use crate::domain::GameId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TurnOrigin {
    Discord,
    Ingame,
    Deferred,
    Operator,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Environment {
    Production,
    Operator,
    Development,
}

impl Environment {
    fn for_origin(deployment: Self, origin: TurnOrigin) -> Self {
        match origin {
            TurnOrigin::Operator => Self::Operator,
            TurnOrigin::Discord
            | TurnOrigin::Ingame
            | TurnOrigin::Deferred
            | TurnOrigin::Watchdog => deployment,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Production => "production",
            Self::Operator => "operator",
            Self::Development => "development",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TurnActor {
    Discord(u64),
    Player(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TurnIdentity {
    pub(crate) origin: TurnOrigin,
    pub(crate) deployment: Environment,
    pub(crate) guild: Option<u64>,
    pub(crate) channel: Option<u64>,
    pub(crate) actor: TurnActor,
    pub(crate) access: AccessLevel,
    pub(crate) session: Option<SessionId>,
    pub(crate) game: Option<GameId>,
}

impl TurnIdentity {
    pub(crate) fn environment(&self) -> Environment {
        Environment::for_origin(self.deployment, self.origin)
    }
}

#[cfg(test)]
#[path = "tests/turn.rs"]
mod tests;
