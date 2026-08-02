//! Modrinth API v2 client, backing Gary's mod-discovery tools.
//!
//! This exists so mod identification is grounded in the live registry rather
//! than the model's training data. A hallucinated slug is not a harmless miss:
//! itzg's entrypoint re-resolves `MODRINTH_PROJECTS` on every container start
//! and *hard-fails the boot* on a bad entry, and a bad value that nothing rolls
//! back is re-read by the recreated pod off the same volume, so the server
//! crash-loops identically. Looking a slug up before writing it is what closes
//! that hole.
//!
//! The split follows the house shape: [`query`] and [`types`] are pure — facet
//! construction, response shapes, and the bounding applied to third-party text —
//! while [`client`] is a thin shell that signs nothing, holds nothing, and only
//! builds a request, checks the status, and hands the body to those types.
//!
//! Two quirks of v2 shape the types and are worth knowing before editing them:
//! datapacks are not a project type (they are `project_type: "mod"` carrying
//! `datapack` among their loaders), and a search hit has no `loaders` field at
//! all — its loaders are mixed into `categories` alongside real tags. A project
//! lookup, by contrast, returns clean `loaders` and `game_versions` arrays. That
//! is why search and lookup deserialize into two different structs rather than
//! one shared one.

pub(crate) mod client;
pub(crate) mod query;
pub(crate) mod types;

pub(crate) use client::{fetch_project, search_projects};
pub(crate) use query::{Kind, Loader, SearchQuery};
pub(crate) use types::{
    Project, SearchHit, list_or_none, recent_versions, short_description, short_title,
};
