//! Error type for Twin operations.

use std::path::PathBuf;
use thiserror::Error;

/// Errors produced by `lunco-twin` operations.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TwinError {
    /// An I/O error while reading or writing a file.
    #[error("I/O error on {path}: {source}")]
    Io {
        /// The path being accessed when the error occurred.
        path: PathBuf,
        /// The underlying `std::io::Error`.
        #[source]
        source: std::io::Error,
    },

    /// Directory traversal error (from `walkdir`).
    #[error("directory walk error: {0}")]
    WalkDir(String),

    /// An open path is neither a file nor a directory (e.g. a device
    /// node, broken symlink).
    #[error("path is neither a file nor a directory: {0}")]
    NotAFileOrFolder(PathBuf),

    /// While indexing, a discovered path could not be made relative to
    /// the Twin root. Indicates a logic bug in `walkdir` usage.
    #[error("path {path} is outside of Twin root {root}")]
    PathOutsideRoot {
        /// The offending absolute path.
        path: PathBuf,
        /// The Twin root the path should have been inside.
        root: PathBuf,
    },

    /// Attempted to save a manifest on a Twin that doesn't have one.
    /// Call `promote_to_twin` first.
    #[error("Twin has no manifest (call promote_to_twin first)")]
    NoManifest,

    /// `twin.toml` failed to parse as TOML.
    #[error("failed to parse twin.toml: {0}")]
    ManifestParse(#[from] toml::de::Error),

    /// A manifest failed to serialize to TOML (should be impossible for
    /// well-formed `TwinManifest` structs, but surfaced for completeness).
    #[error("failed to serialize twin.toml: {0}")]
    ManifestSerialize(#[from] toml::ser::Error),
}
