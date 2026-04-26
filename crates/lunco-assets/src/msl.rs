//! MSL (Modelica Standard Library) asset source — cross-target abstraction.
//!
//! The desktop build reads MSL from disk via [`crate::msl_source_root_path`].
//! The web build fetches a versioned bundle (`dist/<bin>/msl/`) and unpacks
//! it into memory. Consumers in `lunco-modelica` (the rumoca compile path,
//! the `modelica://` image loader, etc.) read through [`MslAssetSource`]
//! instead of touching `std::fs` directly.
//!
//! This module owns the *types* — the actual web fetch lives in
//! `lunco-modelica/src/msl_remote.rs` because that's where the
//! `web-sys`/`wasm-bindgen-futures` deps already are. Keeping
//! `lunco-assets` web-sys-free keeps it cheap to depend on from
//! everywhere else.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Where MSL bytes come from on this target.
///
/// Construct one and insert it as a `Resource`; consumers read through
/// the methods instead of branching on the variant.
#[derive(Resource, Clone, Debug)]
pub enum MslAssetSource {
    /// MSL is materialised on the local filesystem at this root. The
    /// path is what would have been returned by
    /// [`crate::msl_source_root_path`] — i.e. the parent of `Modelica/`.
    Filesystem(PathBuf),
    /// MSL has been fetched and decompressed into memory. Used on
    /// `wasm32-unknown-unknown`. Populated by the remote-load plugin
    /// after the bundle download finishes.
    InMemory(Arc<MslInMemory>),
}

/// Decompressed MSL tree held entirely in memory, keyed by
/// MSL-relative path (e.g. `Modelica/Blocks/PID.mo`).
#[derive(Debug)]
pub struct MslInMemory {
    pub files: HashMap<PathBuf, Vec<u8>>,
}

impl MslInMemory {
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn total_bytes(&self) -> u64 {
        self.files.values().map(|v| v.len() as u64).sum()
    }

    /// Materialise as `(uri, source)` pairs for
    /// `Session::load_source_root_in_memory`. URIs are MSL-relative
    /// paths rendered as forward-slash strings (rumoca treats these as
    /// stable identifiers, not real filesystem paths). Files whose
    /// bytes aren't valid UTF-8 are skipped — Modelica source must be
    /// UTF-8 per MLS §13.5.
    pub fn as_source_pairs(&self) -> Vec<(String, String)> {
        let mut out = Vec::with_capacity(self.files.len());
        for (path, bytes) in &self.files {
            let uri = path.to_string_lossy().replace('\\', "/");
            match std::str::from_utf8(bytes) {
                Ok(s) => out.push((uri, s.to_string())),
                Err(_) => {
                    // Non-UTF8 (stray binary); skip but keep going.
                }
            }
        }
        out
    }
}

/// Global, write-once handle to the active [`MslAssetSource`] for this
/// process.
///
/// Why a global: `ModelicaCompiler::new()` is called from many sites
/// (lazy `get_or_insert_with`, integration tests, etc.) and threading a
/// resource through each is invasive. A `OnceLock` keeps the contract
/// crisp — the source is set exactly once during boot and is read by
/// any compile init that follows.
///
/// On native this is set immediately at app build (the disk path is
/// always available synchronously). On wasm it's set by the
/// `MslRemotePlugin` drain once the bundle has been fetched and
/// decompressed; compiles dispatched before that point will see `None`
/// and start with an empty session.
static GLOBAL_MSL_SOURCE: OnceLock<MslAssetSource> = OnceLock::new();

/// Install the process-wide MSL source. Call once during boot.
/// Subsequent calls are silently ignored — the contract is set-once.
pub fn install_global_msl_source(source: MslAssetSource) {
    let _ = GLOBAL_MSL_SOURCE.set(source);
}

/// Read the process-wide MSL source if any has been installed.
pub fn global_msl_source() -> Option<&'static MslAssetSource> {
    GLOBAL_MSL_SOURCE.get()
}

impl MslAssetSource {
    /// Read a single MSL file's bytes by relative path. `None` if absent.
    /// Sync on both targets — the in-memory branch hands back a slice;
    /// the filesystem branch does a `std::fs::read`.
    pub fn read(&self, rel: &std::path::Path) -> Option<Vec<u8>> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            MslAssetSource::Filesystem(root) => std::fs::read(root.join(rel)).ok(),
            // On wasm we never construct Filesystem; this match arm exists
            // so the type is the same shape on both targets.
            #[cfg(target_arch = "wasm32")]
            MslAssetSource::Filesystem(_) => None,
            MslAssetSource::InMemory(inner) => inner.files.get(rel).cloned(),
        }
    }
}

/// Boot-time loading state for [`MslAssetSource`]. UI watches this to
/// surface progress; compile triggers can gate on `Ready`.
///
/// On native this resource is inserted as `Ready` immediately when MSL is
/// already on disk, or `Failed` when it isn't. On wasm it starts in
/// `Loading` while the fetch task runs.
#[derive(Resource, Debug, Clone)]
pub enum MslLoadState {
    NotStarted,
    Loading {
        phase: MslLoadPhase,
        /// `0..bytes_total`. `bytes_total` of `0` means "unknown".
        bytes_done: u64,
        bytes_total: u64,
    },
    Ready {
        file_count: usize,
        compressed_bytes: u64,
        uncompressed_bytes: u64,
    },
    Failed(String),
}

impl Default for MslLoadState {
    fn default() -> Self {
        MslLoadState::NotStarted
    }
}

/// Phases of remote MSL load. Surfaced in UI as a single status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MslLoadPhase {
    FetchingManifest,
    FetchingBundle,
    Decompressing,
    /// Per-file AST parse, chunked across frames. `bytes_done` /
    /// `bytes_total` in the surrounding `Loading` variant carry the
    /// file count rather than byte count for this phase.
    Parsing,
}

impl MslLoadPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            MslLoadPhase::FetchingManifest => "fetching manifest",
            MslLoadPhase::FetchingBundle => "fetching MSL",
            MslLoadPhase::Decompressing => "decompressing",
            MslLoadPhase::Parsing => "parsing MSL",
        }
    }
}

/// Schema of `manifest.json` written by `build_msl_assets`. Kept here
/// (not in the build binary) so both producer and consumer share the type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MslManifest {
    pub schema_version: u32,
    pub sources: MslBundleEntry,
    /// Pre-parsed bundle (`Vec<(String, StoredDefinition)>` bincode'd).
    /// Optional: when present the wasm runtime fetches and deserialises
    /// it instead of parsing source files (which is unworkably slow
    /// on wasm — ~600 ms/file × 2670 files ≈ 27 minutes). The encoding
    /// is bincode 1.3 and must match the rumoca version this artifact
    /// was produced against; the manifest carries `rumoca_artifact_tag`
    /// for the runtime to validate.
    #[serde(default)]
    pub parsed: Option<MslBundleEntry>,
    /// Free-form tag identifying the rumoca version that produced the
    /// `parsed` bundle. The runtime compares against its own compiled-in
    /// tag and refuses to load a mismatched bundle (the encoded
    /// `StoredDefinition` shape is rumoca-version-sensitive).
    #[serde(default)]
    pub rumoca_artifact_tag: Option<String>,
    /// Hint to the runtime that the unpacked tree should contain this
    /// file under its root. Used as a sanity check after extraction.
    pub msl_root_marker: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MslBundleEntry {
    pub filename: String,
    pub sha256: String,
    pub uncompressed_bytes: u64,
    pub compressed_bytes: u64,
    pub file_count: usize,
}
