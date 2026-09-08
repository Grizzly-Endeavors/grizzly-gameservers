//! Registry-agnostic pieces of mod discovery: the bounding applied to
//! third-party registry text, and the side vocabulary the two registries express
//! differently but Gary asks about identically.
//!
//! Modrinth and Thunderstore disagree about almost everything — one has slugs
//! and typed `server_side`/`client_side` fields, the other has `Namespace-Name`
//! pairs and per-community category slugs — but the *questions* Gary asks are
//! the same in both: what is this, and does anyone besides the server have to
//! install it. What lives here is the part of the answer neither registry owns.

pub(crate) mod side;
pub(crate) mod text;

pub(crate) use side::{Side, SideSupport};
pub(crate) use text::{list_or_none, short_description, short_title};
