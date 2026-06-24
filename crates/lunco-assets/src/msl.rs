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
#[derive(Clone, Debug)]
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
static GLOBAL_MSL_SOURCES: OnceLock<Vec<MslAssetSource>> = OnceLock::new();

/// Install the process-wide, ordered list of library roots. Call once
/// during boot; subsequent calls are silently ignored (set-once).
///
/// Roots are searched in order — MSL first, then any extra libraries —
/// so resolution and reads prefer earlier roots. Each root is a single
/// backend: `Filesystem` on native, `InMemory` on web. The list may mix
/// backends, though in practice a target is homogeneous.
pub fn install_global_msl_sources(sources: Vec<MslAssetSource>) {
    let _ = GLOBAL_MSL_SOURCES.set(sources);
}

/// The process-wide ordered library roots. Empty slice if none have
/// been installed yet (e.g. web boot before the fetch completes).
pub fn global_msl_sources() -> &'static [MslAssetSource] {
    GLOBAL_MSL_SOURCES.get().map(Vec::as_slice).unwrap_or(&[])
}

/// `true` once at least one root is installed.
pub fn has_msl_source() -> bool {
    !global_msl_sources().is_empty()
}

/// Read a relative (or root-joined) path's bytes from the first root
/// that has it.
pub fn msl_read(rel: &std::path::Path) -> Option<Vec<u8>> {
    global_msl_sources().iter().find_map(|s| s.read(rel))
}

/// `true` if any installed root is an in-memory bundle (web). The
/// compiler-init gate uses this to avoid blocking the main thread on a
/// synchronous parse.
pub fn has_in_memory_source() -> bool {
    global_msl_sources()
        .iter()
        .any(|s| matches!(s, MslAssetSource::InMemory(_)))
}

/// The first filesystem root's directory (the MSL tree), if any. The
/// native engine-seed parse path needs the MSL dir specifically.
pub fn primary_filesystem_root() -> Option<&'static std::path::Path> {
    global_msl_sources().iter().find_map(|s| match s {
        MslAssetSource::Filesystem(p) => Some(p.as_path()),
        MslAssetSource::InMemory(_) => None,
    })
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

    /// The base path candidate paths are joined onto for this root.
    /// `Filesystem` → the on-disk root dir (candidates are absolute);
    /// `InMemory` → empty (candidates are bundle-relative keys). The
    /// `library_fs` resolver uses this to build §13 candidate paths
    /// without knowing the backend.
    pub fn base(&self) -> &std::path::Path {
        match self {
            MslAssetSource::Filesystem(root) => root.as_path(),
            MslAssetSource::InMemory(_) => std::path::Path::new(""),
        }
    }

    /// Does this root contain `candidate` (a path already joined onto
    /// [`base`](Self::base))? In-memory → a map lookup; filesystem →
    /// membership in a resident path-set built once by a single
    /// sequential directory walk (see [`native_path_set`]), so the
    /// resolver never `stat()`s per candidate. This keeps ALL native
    /// filesystem traversal inside `lunco-assets`.
    pub fn contains(&self, candidate: &std::path::Path) -> bool {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            MslAssetSource::Filesystem(_) => native_path_set().contains(candidate),
            #[cfg(target_arch = "wasm32")]
            MslAssetSource::Filesystem(_) => false,
            MslAssetSource::InMemory(inner) => inner.files.contains_key(candidate),
        }
    }
}

/// Resident set of every `.mo` file's absolute path across all
/// installed *filesystem* roots (MSL + extra libraries). Built once,
/// lazily, by a single sequential walk; replaces per-candidate
/// `stat()` (the "stat-storm") with O(1) membership.
///
/// Candidates are absolute and root-prefixed, so a single union set
/// over all roots is unambiguous. An *empty* result (no filesystem
/// root installed yet — early boot / not-yet-fetched background
/// install) is returned WITHOUT memoising, so a later call retries
/// once the roots land.
#[cfg(not(target_arch = "wasm32"))]
fn native_path_set() -> &'static std::collections::HashSet<PathBuf> {
    static SET: OnceLock<std::collections::HashSet<PathBuf>> = OnceLock::new();
    static EMPTY: OnceLock<std::collections::HashSet<PathBuf>> = OnceLock::new();

    if let Some(set) = SET.get() {
        return set;
    }
    let has_fs_root = global_msl_sources()
        .iter()
        .any(|s| matches!(s, MslAssetSource::Filesystem(_)));
    if !has_fs_root {
        return EMPTY.get_or_init(std::collections::HashSet::new);
    }
    SET.get_or_init(build_native_path_set)
}

#[cfg(not(target_arch = "wasm32"))]
fn build_native_path_set() -> std::collections::HashSet<PathBuf> {
    let start = std::time::Instant::now();
    let mut files = std::collections::HashSet::new();
    let mut n_roots = 0usize;
    for source in global_msl_sources() {
        if let MslAssetSource::Filesystem(dir) = source {
            n_roots += 1;
            collect_mo_files(dir, &mut files);
        }
    }
    info!(
        "[Msl] native path-set: {} .mo files across {} roots in {:?}",
        files.len(),
        n_roots,
        start.elapsed()
    );
    files
}

/// Recursively insert the absolute path of every `*.mo` file under
/// `dir` (including `package.mo`). Skips MSL's non-source dirs.
#[cfg(not(target_arch = "wasm32"))]
fn collect_mo_files(dir: &std::path::Path, files: &mut std::collections::HashSet<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if matches!(name, "Resources" | "Images" | "test") {
                    continue;
                }
            }
            collect_mo_files(&path, files);
        } else if file_type.is_file()
            && path.extension().and_then(|e| e.to_str()) == Some("mo")
        {
            files.insert(path);
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

impl MslLoadState {
    /// True once the MSL bundle is resident and resolvable (the single
    /// readiness predicate; the inverse of [`is_pending`](Self::is_pending)
    /// plus the terminal `Failed` state). Prefer this over hand-rolled
    /// `matches!(.., Ready { .. })` at call sites so the readiness rule
    /// lives in one place.
    pub fn is_ready(&self) -> bool {
        matches!(self, MslLoadState::Ready { .. })
    }

    /// True while MSL is still on its way to `Ready` — i.e. not yet started,
    /// or actively loading. `Failed` is **not** pending (it will never
    /// become ready), so callers that want "still arriving" semantics get
    /// `false` for a failed load.
    pub fn is_pending(&self) -> bool {
        matches!(self, MslLoadState::NotStarted | MslLoadState::Loading { .. })
    }
}

/// Phases of remote MSL load. Surfaced in UI as a single status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MslLoadPhase {
    FetchingManifest,
    FetchingBundle,
    /// Bundle served from the browser's Cache Storage — no network download.
    /// Distinct from [`FetchingBundle`](Self::FetchingBundle) so the status
    /// line doesn't say "downloading" on a warm reload (the bundle is
    /// content-hashed + cached-first-forever).
    LoadingCache,
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
            MslLoadPhase::FetchingBundle => "downloading MSL",
            MslLoadPhase::LoadingCache => "loading MSL from cache",
            MslLoadPhase::Decompressing => "decompressing",
            MslLoadPhase::Parsing => "parsing MSL",
        }
    }
}

/// The rumoca-artifact tag the *current* build understands. `build_msl_assets`
/// stamps it into `manifest.rumoca_artifact_tag`; the web runtime compares
/// against it and declines a mismatched `parsed` bundle (falling back to a
/// source parse) because the bincode'd `StoredDefinition` layout is
/// rumoca-version-sensitive — a stale bundle would deserialize into garbage or
/// error mid-load.
///
/// BUMP THIS whenever the pinned rumoca changes its AST / `StoredDefinition`
/// shape. Producer (`build_msl_assets`) and consumer (`msl_remote`) share this
/// one source of truth, so they can never drift apart.
pub const EXPECTED_RUMOCA_ARTIFACT_TAG: &str = "rumoca-main-2026-06-15";

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
