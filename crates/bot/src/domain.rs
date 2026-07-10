//! Domain identity newtypes shared by the agones and backup layers.
//!
//! A server's owning guild, its instance name, and its catalog game are all
//! strings that travel together — through `InstanceIdentity`, the S3
//! `BackupManifest`, the `ArchiveRecord` index row, and the `manifest` builder
//! that stamps every artifact. Where three same-typed `&str` sit adjacent, a
//! transposition (`game` for `guild`, `instance` for `created_by`) compiles
//! clean and mislabels the artifact for good. Wrapping each in a distinct type
//! turns that swap into a compile error.
//!
//! The wrappers are `#[serde(transparent)]`, so anything that serializes them
//! (the backup manifest sidecar) keeps its existing bare-string wire format —
//! this is a type-level change only, never a format change.

use serde::{Deserialize, Serialize};

macro_rules! string_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(transparent)]
        pub(crate) struct $name(String);

        impl $name {
            /// Wrap a string as this identity. Takes anything string-like so a
            /// `&str` label value or an owned `String` both flow in cleanly.
            pub(crate) fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Borrow the underlying string for a k8s label, SQL bind, or object
            /// key — the boundaries that still speak in `&str`.
            pub(crate) fn as_str(&self) -> &str {
                &self.0
            }

            /// Consume into the owned string, for a boundary that needs `String`
            /// (a display DTO field, a `String`-typed outcome variant).
            pub(crate) fn into_string(self) -> String {
                self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_id! {
    /// Discord guild (server) id that owns an instance — the tenant an artifact
    /// belongs to and the guild a recovered server is stamped back into.
    GuildId
}

string_id! {
    /// A game-server instance (world) name — the per-world identifier used for
    /// the k8s object names, the archive key segment, and the recover handle.
    InstanceName
}

string_id! {
    /// A catalog game id (`minecraft`, `valheim`, ...) — which game an instance
    /// runs, used to look the template up in the catalog.
    GameId
}

#[cfg(test)]
#[path = "tests/domain.rs"]
mod tests;
