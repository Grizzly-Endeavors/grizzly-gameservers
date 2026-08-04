//! The event vocabulary: what happened, who did it, and which surface they did
//! it from.
//!
//! The three enums here are **closed sets** whose `as_str` renderings land in the
//! `events` table and are queried by the derivation layer, so they are a
//! published vocabulary rather than a rendering detail — the same contract
//! [`crate::agent::Environment`] carries for Langfuse. Adding a variant is a
//! schema change in everything but name.

use crate::agent::{TurnActor, TurnOrigin};
use crate::domain::{GameId, GuildId, InstanceName};

/// A product-meaningful action worth a permanent record. One variant per row in
/// the design's event catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EventKind {
    ServerCreated,
    ServerStarted,
    ServerStopped,
    ServerRestarted,
    /// The `GameServer` was deleted but the world's PVC kept — a cold `/start`
    /// brings it back.
    ServerShutdown,
    ServerDestroyed,
    ServerArchived,
    ServerRecovered,
    /// An archive was deleted — the tarball, its manifest, and the index rows.
    /// Distinct from `ServerDestroyed`: destroying a server deliberately leaves
    /// its archives standing, so this is the only kind that records a world
    /// becoming unrecoverable.
    ArchiveDeleted,
    BackupTaken,
    ServerRestored,
    /// The scheduled snapshot cycle finished a pass.
    BackupCycle,
    /// Gary gave up — round budget spent, or a crash rollback that didn't recover.
    Escalation,
    /// First-ever sighting of a guild, backed by the known-guilds table so a
    /// gateway reconnect can't re-fire it.
    GuildJoined,
    /// The proactive watchdog detected a problem on a running server. One row
    /// per fired incident whatever its disposition (shadow, responded,
    /// suppressed), so detection is auditable even when nothing acted.
    Incident,
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
            Self::GuildJoined => "guild_joined",
            Self::Incident => "incident",
        }
    }
}

/// Where an action was driven from. Deliberately *not* [`TurnOrigin`]: origin
/// answers "how did a Gary turn arrive", surface answers "what drove this
/// action" — and two of these have no turn behind them at all (a slash command,
/// an automatic cycle) while [`Surface::Agent`] collapses the two production
/// Gary origins that are equally "the model did it".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Surface {
    /// A slash command a friend ran.
    Command,
    /// A tool inside a production Gary turn — Discord or in-game.
    Agent,
    /// A tool inside a deferred (`run_when`) batch.
    Deferred,
    /// Anything driven through the operator harness, whether a direct tool
    /// dispatch or a tool inside an operator-origin turn. Separable here for the
    /// same reason the `operator` environment is separable in Langfuse: harness
    /// traffic must never count as product signal.
    Operator,
    /// A tool inside a proactive watchdog turn — the model acting on its own
    /// detection, with no asker on any surface. Not [`Surface::System`] (that
    /// vocabulary means "no model behind it") and not [`Surface::Deferred`]
    /// (a friend queued those; nobody queued this).
    Watchdog,
    /// An automatic cycle — scheduled backups, stranded-instance recovery.
    System,
}

impl Surface {
    /// The surface a Gary tool call belongs to, given the turn it runs inside.
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
        }
    }
}

/// Who acted. In-game players have no Discord identity, so their name is
/// prefixed on the way into the column — otherwise a player called `12345` would
/// be indistinguishable from a snowflake, and one called `system` from an
/// automatic action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Actor {
    Discord(u64),
    Player(String),
    /// An action nobody asked for: a scheduled cycle or a reconciler.
    System,
}

impl Actor {
    /// Who triggered a Gary turn, as an event actor.
    pub(crate) fn from_turn(actor: &TurnActor) -> Self {
        match actor {
            TurnActor::Discord(id) => Self::Discord(*id),
            TurnActor::Player(name) => Self::Player(name.clone()),
        }
    }

    /// The `actor` column value.
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Discord(id) => id.to_string(),
            Self::Player(name) => format!("player:{name}"),
            Self::System => "system".to_owned(),
        }
    }
}

/// Who did it and what drove them — the attribution every event row carries, and
/// the thing lifecycle orchestration has always dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Attribution {
    pub(crate) actor: Actor,
    pub(crate) surface: Surface,
}

impl Attribution {
    pub(crate) fn new(actor: Actor, surface: Surface) -> Self {
        Self { actor, surface }
    }

    /// An automatic action: no actor, driven by a cycle.
    pub(crate) fn system() -> Self {
        Self::new(Actor::System, Surface::System)
    }
}

/// One row bound for the `events` table. `guild`, `game`, and `instance` are each
/// optional because the catalog is one wide table: a `guild_joined` has no
/// server, a `backup_cycle` has no guild.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProductEvent {
    pub(crate) kind: EventKind,
    pub(crate) guild: Option<GuildId>,
    pub(crate) game: Option<GameId>,
    pub(crate) instance: Option<InstanceName>,
    pub(crate) actor: Actor,
    pub(crate) surface: Surface,
    /// How the action ended, from [`super::mapping`] — never free text, so the
    /// derivation layer can group on it.
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
