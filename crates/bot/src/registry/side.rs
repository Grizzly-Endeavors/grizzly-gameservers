//! Which machines a mod has to be installed on — the one compatibility question
//! a non-technical friend actually feels, since anything needed client-side is
//! work no server-side change can do for them.
//!
//! The two registries model it differently and neither models it well:
//! Modrinth carries typed `server_side`/`client_side` support fields, while
//! Thunderstore has no flag at all and the communities express it with
//! categories (`server-side`, `client-side`). Both collapse into the same
//! four-way [`SideSupport`], and Gary filters with the same three-way [`Side`].

/// The side filter a search may narrow on. Read as "applies to", not "applies
/// only to": a mod needed on both machines answers a `Server` search, because
/// it does need installing on the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    /// Installed on the server.
    Server,
    /// Installed on the players' own machines.
    Client,
    /// Installed on both, so every player has work to do.
    Both,
}

impl From<crate::prompts::SearchModsSide> for Side {
    fn from(value: crate::prompts::SearchModsSide) -> Self {
        match value {
            crate::prompts::SearchModsSide::Server => Self::Server,
            crate::prompts::SearchModsSide::Client => Self::Client,
            crate::prompts::SearchModsSide::Both => Self::Both,
        }
    }
}

/// Where a package has to be installed, as far as the registry says.
///
/// [`Unknown`](SideSupport::Unknown) is a real answer and not a defaulting
/// convenience: a Thunderstore package whose author tagged neither category is
/// genuinely unclassified, and reporting that is what sends Gary to the mod page
/// instead of guessing "server only" and leaving players unable to connect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SideSupport {
    ServerOnly,
    ClientOnly,
    Both,
    Unknown,
}

impl SideSupport {
    /// Classify from the two independent "applies here" flags each registry
    /// exposes in its own vocabulary.
    pub(crate) fn from_flags(server: bool, client: bool) -> Self {
        match (server, client) {
            (true, true) => Self::Both,
            (true, false) => Self::ServerOnly,
            (false, true) => Self::ClientOnly,
            (false, false) => Self::Unknown,
        }
    }

    /// Whether a package with this support answers a search narrowed to `side`.
    ///
    /// `Unknown` matches nothing: an unclassified package surviving a `server`
    /// filter would read as "the registry says this is server-side", which is
    /// the one thing it does not say.
    pub(crate) fn matches(self, side: Side) -> bool {
        match side {
            Side::Server => matches!(self, Self::ServerOnly | Self::Both),
            Side::Client => matches!(self, Self::ClientOnly | Self::Both),
            Side::Both => matches!(self, Self::Both),
        }
    }

    /// The plain-language phrase rendered into a result line. A value-level
    /// rendering of the classification rather than instruction-bearing prose, so
    /// it stays in Rust — each consuming prompt file documents it under the
    /// variable's `contents`.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ServerOnly => "the server only",
            Self::ClientOnly => "players' machines only — nothing to install on the server",
            Self::Both => "the server and every player's machine",
            Self::Unknown => "not stated by the registry — check the mod page",
        }
    }
}

#[cfg(test)]
#[path = "tests/side.rs"]
mod tests;
