use crate::agent::{TurnActor, TurnOrigin};
use crate::domain::{GameId, GuildId, InstanceName};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EventKind {
    ServerCreated,
    ServerStarted,
    ServerStopped,
    ServerRestarted,
    ServerShutdown,
    ServerDestroyed,
    ServerArchived,
    ServerRecovered,
    ArchiveDeleted,
    BackupTaken,
    ServerRestored,
    BackupCycle,
    Escalation,
    TurnCancelled,
    GuildJoined,
    BetaApproved,
    BetaDenied,
    SignupEmailed,
    Incident,
    ResourcesChanged,
    SettingChanged,
}

impl EventKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ServerCreated => "server_created",
            Self::ServerStarted => "server_started",
            Self::ServerStopped => "server_stopped",
            Self::ServerRestarted => "server_restarted",
            Self::ServerShutdown => "server_shutdown",
            Self::ServerDestroyed => "server_destroyed",
            Self::ServerArchived => "server_archived",
            Self::ServerRecovered => "server_recovered",
            Self::ArchiveDeleted => "archive_deleted",
            Self::BackupTaken => "backup_taken",
            Self::ServerRestored => "server_restored",
            Self::BackupCycle => "backup_cycle",
            Self::Escalation => "escalation",
            Self::TurnCancelled => "turn_cancelled",
            Self::GuildJoined => "guild_joined",
            Self::BetaApproved => "beta_approved",
            Self::BetaDenied => "beta_denied",
            Self::SignupEmailed => "signup_emailed",
            Self::Incident => "incident",
            Self::ResourcesChanged => "resources_changed",
            Self::SettingChanged => "setting_changed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Surface {
    Command,
    Agent,
    Deferred,
    Operator,
    Watchdog,
    System,
    Admin,
}

impl Surface {
    pub(crate) fn from_origin(origin: TurnOrigin) -> Self {
        match origin {
            TurnOrigin::Discord | TurnOrigin::Ingame => Self::Agent,
            TurnOrigin::Deferred => Self::Deferred,
            TurnOrigin::Operator => Self::Operator,
            TurnOrigin::Watchdog => Self::Watchdog,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::Agent => "agent",
            Self::Deferred => "deferred",
            Self::Operator => "operator",
            Self::Watchdog => "watchdog",
            Self::System => "system",
            Self::Admin => "admin",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Actor {
    Discord(u64),
    Player(String),
    System,
    Admin(String),
}

impl Actor {
    pub(crate) fn from_turn(actor: &TurnActor) -> Self {
        match actor {
            TurnActor::Discord(id) => Self::Discord(*id),
            TurnActor::Player(name) => Self::Player(name.clone()),
        }
    }

    pub(crate) fn label(&self) -> String {
        match self {
            Self::Discord(id) => id.to_string(),
            Self::Player(name) => format!("player:{name}"),
            Self::System => "system".to_owned(),
            Self::Admin(username) => format!("admin:{username}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Attribution {
    pub(crate) actor: Actor,
    pub(crate) surface: Surface,
}

impl Attribution {
    pub(crate) fn new(actor: Actor, surface: Surface) -> Self {
        Self { actor, surface }
    }

    pub(crate) fn system() -> Self {
        Self::new(Actor::System, Surface::System)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProductEvent {
    pub(crate) kind: EventKind,
    pub(crate) guild: Option<GuildId>,
    pub(crate) game: Option<GameId>,
    pub(crate) instance: Option<InstanceName>,
    pub(crate) actor: Actor,
    pub(crate) surface: Surface,
    pub(crate) outcome: &'static str,
    pub(crate) details: serde_json::Value,
}

impl ProductEvent {
    pub(crate) fn new(kind: EventKind, attribution: &Attribution, outcome: &'static str) -> Self {
        Self {
            kind,
            guild: None,
            game: None,
            instance: None,
            actor: attribution.actor.clone(),
            surface: attribution.surface,
            outcome,
            details: serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    pub(crate) fn guild(mut self, guild: Option<GuildId>) -> Self {
        self.guild = guild;
        self
    }

    pub(crate) fn game(mut self, game: Option<GameId>) -> Self {
        self.game = game;
        self
    }

    pub(crate) fn instance(mut self, instance: InstanceName) -> Self {
        self.instance = Some(instance);
        self
    }

    pub(crate) fn details(mut self, details: serde_json::Value) -> Self {
        self.details = details;
        self
    }
}

#[cfg(test)]
#[path = "tests/catalog.rs"]
mod tests;
