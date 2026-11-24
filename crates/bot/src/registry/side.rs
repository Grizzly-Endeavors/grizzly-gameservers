#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Server,
    Client,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SideSupport {
    ServerOnly,
    ClientOnly,
    Both,
    Unknown,
}

impl SideSupport {
    pub(crate) fn from_flags(server: bool, client: bool) -> Self {
        match (server, client) {
            (true, true) => Self::Both,
            (true, false) => Self::ServerOnly,
            (false, true) => Self::ClientOnly,
            (false, false) => Self::Unknown,
        }
    }

    pub(crate) fn matches(self, side: Side) -> bool {
        match side {
            Side::Server => matches!(self, Self::ServerOnly | Self::Both),
            Side::Client => matches!(self, Self::ClientOnly | Self::Both),
            Side::Both => matches!(self, Self::Both),
        }
    }

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
