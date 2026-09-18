//! Cross-target source-library storage and bundle contracts.
//!
//! A source library can be materialised on the host filesystem or kept as an
//! in-memory tree on wasm. Consumers use the same read and membership methods
//! for both backends; library-specific loaders own fetching, parsing, and
//! presentation.
//!
//! This module deliberately does not identify a particular Modelica library.
//! It owns only the reusable source-store and bundle-envelope mechanisms. The
//! Modelica integration supplies the library identity and decides when to load
//! it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Where a source library is materialised on the current target.
///
/// Construct one and install it during application setup. Readers use the
/// methods on this type instead of branching on the target or backend.
#[derive(Clone, Debug)]
pub enum LibrarySource {
    /// The library is materialised on the local filesystem at this root.
    Filesystem(PathBuf),
    /// The library has been fetched and decompressed into memory.
    InMemory(Arc<InMemoryLibrary>),
}

/// Decompressed source-library tree held entirely in memory.
#[derive(Debug)]
pub struct InMemoryLibrary {
    /// Files keyed by library-relative path.
    pub files: HashMap<PathBuf, Vec<u8>>,
}

impl InMemoryLibrary {
    /// Number of files in the tree.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Total size of the decompressed files.
    pub fn total_bytes(&self) -> u64 {
        self.files.values().map(|v| v.len() as u64).sum()
    }

    /// Materialise UTF-8 files as stable `(uri, source)` pairs.
    ///
    /// URIs are library-relative paths rendered with forward slashes. Invalid
    /// UTF-8 is skipped because Modelica source is UTF-8 text.
    pub fn as_source_pairs(&self) -> Vec<(String, String)> {
        let mut out = Vec::with_capacity(self.files.len());
        for (path, bytes) in &self.files {
            let uri = lunco_assets_path::slashed(path);
            if let Ok(source) = std::str::from_utf8(bytes) {
                out.push((uri, source.to_string()));
            }
        }
        out
    }
}

/// Process-wide, ordered source-library roots.
///
/// This is installed once during boot. The source list may contain multiple
/// libraries and may mix filesystem and in-memory backends. The Modelica
/// loader owns the admission policy; this store only provides read-side
/// access to the installed roots.
static GLOBAL_LIBRARY_SOURCES: OnceLock<Vec<LibrarySource>> = OnceLock::new();

/// Install the process-wide, ordered list of source-library roots.
pub fn install_global_library_sources(sources: Vec<LibrarySource>) {
    let _ = GLOBAL_LIBRARY_SOURCES.set(sources);
}

/// Return the process-wide ordered source-library roots.
pub fn global_library_sources() -> &'static [LibrarySource] {
    GLOBAL_LIBRARY_SOURCES
        .get()
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// Return whether at least one source-library root is installed.
pub fn has_library_source() -> bool {
    !global_library_sources().is_empty()
}

/// Return whether `path` belongs to an installed filesystem-backed library.
#[cfg(not(target_arch = "wasm32"))]
pub fn owns_filesystem_path(path: &std::path::Path) -> bool {
    global_library_sources()
        .iter()
        .any(|source| matches!(source, LibrarySource::Filesystem(root) if path.starts_with(root)))
}

/// wasm has no filesystem-backed source roots.
#[cfg(target_arch = "wasm32")]
pub fn owns_filesystem_path(_path: &std::path::Path) -> bool {
    false
}

/// Read a relative path from the first installed source that contains it.
pub fn library_read(rel: &std::path::Path) -> Option<Vec<u8>> {
    global_library_sources()
        .iter()
        .find_map(|source| source.read(rel))
}

/// Open a relative file from the first installed filesystem source that
/// contains it, returning the resolved path for diagnostics.
#[cfg(not(target_arch = "wasm32"))]
pub fn library_open(rel: &std::path::Path) -> Option<(PathBuf, std::fs::File)> {
    global_library_sources()
        .iter()
        .find_map(|source| match source {
            LibrarySource::Filesystem(root) => {
                let path = root.join(rel);
                std::fs::File::open(&path).ok().map(|file| (path, file))
            }
            LibrarySource::InMemory(_) => None,
        })
}

/// Return whether an installed source is in memory.
pub fn has_in_memory_library() -> bool {
    global_library_sources()
        .iter()
        .any(|source| matches!(source, LibrarySource::InMemory(_)))
}

/// Return the first filesystem-backed library root, if any.
pub fn primary_filesystem_library_root() -> Option<&'static std::path::Path> {
    global_library_sources()
        .iter()
        .find_map(|source| match source {
            LibrarySource::Filesystem(path) => Some(path.as_path()),
            LibrarySource::InMemory(_) => None,
        })
}

/// Count the Modelica source files in the installed filesystem roots.
///
/// The resident path set is the shared filesystem traversal used by source
/// membership queries, so callers do not need to maintain a second recursive
/// walker just to populate a load-status count.
#[cfg(not(target_arch = "wasm32"))]
pub fn filesystem_library_file_count() -> usize {
    native_path_set().len()
}

impl LibrarySource {
    /// Read one library-relative file, if present.
    pub fn read(&self, rel: &std::path::Path) -> Option<Vec<u8>> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Self::Filesystem(root) => std::fs::read(root.join(rel)).ok(),
            #[cfg(target_arch = "wasm32")]
            Self::Filesystem(_) => None,
            Self::InMemory(inner) => inner.files.get(rel).cloned(),
        }
    }

    /// Base path used to construct candidate paths for this backend.
    pub fn base(&self) -> &std::path::Path {
        match self {
            Self::Filesystem(root) => root.as_path(),
            Self::InMemory(_) => std::path::Path::new(""),
        }
    }

    /// Return whether this source contains an already-joined candidate path.
    ///
    /// Filesystem membership uses one resident path set rather than a `stat`
    /// per candidate. In-memory membership is a map lookup.
    pub fn contains(&self, candidate: &std::path::Path) -> bool {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Self::Filesystem(_) => native_path_set().contains(candidate),
            #[cfg(target_arch = "wasm32")]
            Self::Filesystem(_) => false,
            Self::InMemory(inner) => inner.files.contains_key(candidate),
        }
    }
}

/// Resident set of every `.mo` file across installed filesystem libraries.
///
/// It is built lazily by one sequential walk. An empty result before a
/// filesystem source is installed is not memoised, allowing a later source
/// installation to retry.
#[cfg(not(target_arch = "wasm32"))]
fn native_path_set() -> &'static std::collections::HashSet<PathBuf> {
    static SET: OnceLock<std::collections::HashSet<PathBuf>> = OnceLock::new();
    static EMPTY: OnceLock<std::collections::HashSet<PathBuf>> = OnceLock::new();

    if let Some(set) = SET.get() {
        return set;
    }
    if !global_library_sources()
        .iter()
        .any(|source| matches!(source, LibrarySource::Filesystem(_)))
    {
        return EMPTY.get_or_init(std::collections::HashSet::new);
    }
    SET.get_or_init(build_native_path_set)
}

#[cfg(not(target_arch = "wasm32"))]
fn build_native_path_set() -> std::collections::HashSet<PathBuf> {
    let start = std::time::Instant::now();
    let mut files = std::collections::HashSet::new();
    let mut root_count = 0usize;
    for source in global_library_sources() {
        if let LibrarySource::Filesystem(root) = source {
            root_count += 1;
            collect_modelica_files(root, &mut files);
        }
    }
    info!(
        "[LibraryFs] native path-set: {} .mo files across {} roots in {:?}",
        files.len(),
        root_count,
        start.elapsed()
    );
    files
}

#[cfg(not(target_arch = "wasm32"))]
fn collect_modelica_files(dir: &std::path::Path, files: &mut std::collections::HashSet<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                if matches!(name, "Resources" | "Images" | "test") {
                    continue;
                }
            }
            collect_modelica_files(&path, files);
        } else if file_type.is_file()
            && path.extension().and_then(|extension| extension.to_str()) == Some("mo")
        {
            files.insert(path);
        }
    }
}

/// Load state for a source-library installation.
#[derive(Resource, Debug, Clone, Default)]
pub enum LibraryLoadState {
    /// No load has been requested.
    #[default]
    NotStarted,
    /// The source or compiled artifact is being loaded.
    Loading {
        /// Current loading phase.
        phase: LibraryLoadPhase,
        /// Completed work. The meaning is phase-specific.
        bytes_done: u64,
        /// Total work, or zero when unknown.
        bytes_total: u64,
    },
    /// The source library is resident and usable.
    Ready {
        /// Number of source files made available.
        file_count: usize,
        /// Compressed artifact size, when applicable.
        compressed_bytes: u64,
        /// Decompressed source size, when applicable.
        uncompressed_bytes: u64,
    },
    /// Installation failed and requires visible user action.
    Failed(String),
}

impl LibraryLoadState {
    /// Return whether the library is ready for use.
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    /// Return whether loading has not reached a terminal state.
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::NotStarted | Self::Loading { .. })
    }
}

/// Phases used by a remote source-library loader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryLoadPhase {
    /// Fetching the bundle manifest.
    FetchingManifest,
    /// Fetching the source or parsed bundle.
    FetchingBundle,
    /// Loading a bundle from browser cache.
    LoadingCache,
    /// Decompressing a downloaded bundle.
    Decompressing,
    /// Parsing source or decoding a parsed artifact.
    Parsing,
}

impl LibraryLoadPhase {
    /// Human-readable status label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FetchingManifest => "fetching manifest",
            Self::FetchingBundle => "downloading library",
            Self::LoadingCache => "loading library from cache",
            Self::Decompressing => "decompressing",
            Self::Parsing => "parsing library",
        }
    }
}

/// Artifact codec/version tag shared by the Modelica library packager and
/// runtime. A mismatch is a packaging error, not a request to use an older
/// decoder.
pub const EXPECTED_RUMOCA_ARTIFACT_TAG: &str = "rumoca-main-2026-07-14+bincode2";

/// Manifest for a source-library bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryManifest {
    /// Manifest schema version.
    pub schema_version: u32,
    /// Compressed source archive metadata.
    pub sources: LibraryBundleEntry,
    /// Optional pre-parsed source artifact metadata.
    pub parsed: LibraryBundleEntry,
    /// Tag identifying the parser/codec that produced `parsed`.
    pub rumoca_artifact_tag: String,
}

/// Metadata for one bundle member.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryBundleEntry {
    /// Bundle filename.
    pub filename: String,
    /// SHA-256 digest of the bundle.
    pub sha256: String,
    /// Uncompressed size.
    pub uncompressed_bytes: u64,
    /// Compressed size.
    pub compressed_bytes: u64,
    /// Number of files in the bundle.
    pub file_count: usize,
}
