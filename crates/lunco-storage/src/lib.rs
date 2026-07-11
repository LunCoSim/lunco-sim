//! # lunco-storage
//!
//! I/O abstraction for LunCoSim.
//!
//! Higher-level crates ([`lunco-doc`](../lunco_doc/index.html),
//! [`lunco-workspace`](../lunco_workspace/index.html),
//! [`lunco-twin`](../lunco_twin/index.html)) never touch the filesystem
//! directly; they go through the [`Storage`] trait against an opaque
//! [`StorageHandle`]. Backends decide what the handle means — a local
//! filesystem path, an IndexedDB key, an OPFS entry, a File-System-Access
//! token, an HTTPS URL, or (in future) an IPFS CID.
//!
//! ## Why the indirection
//!
//! - **Native + web parity** — the same save/load code compiles for
//!   the desktop workbench and a future wasm build. On native the
//!   handle is a `PathBuf`; in a browser it's an IndexedDB key or
//!   an FSA handle. Document code doesn't care.
//! - **Remote twins** — pointing a twin at `https://…` or `ipfs://…`
//!   should "just work" once the appropriate backend exists. No
//!   rewrite of the document layer.
//! - **Testing** — in-memory backend means integration tests for
//!   save/load never touch the real filesystem.
//!
//! ## What ships in v1
//!
//! - The trait + handle enum (this file).
//! - [`FileStorage`] — native POSIX backend using `std::fs` for I/O
//!   and `rfd::FileDialog` for pickers.
//! - Stub variants on [`StorageHandle`] for the future backends so
//!   callers can pattern-match exhaustively when we add them.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::path::{Path, PathBuf};

pub mod file_storage;

pub use file_storage::FileStorage;

/// Browser-`localStorage` backend. Only built for wasm targets — the
/// native build has no `localStorage` and uses [`FileStorage`] instead.
#[cfg(target_arch = "wasm32")]
pub mod web_storage;

#[cfg(target_arch = "wasm32")]
pub use web_storage::WebStorage;

/// OPFS backend for wasm binary assets (meshes/textures/DEMs) — where
/// [`WebStorage`]'s `localStorage`+hex is unusable. Inherent async methods (not
/// the `Send` [`Storage`] trait); see the module docs.
#[cfg(target_arch = "wasm32")]
pub mod opfs_storage;

#[cfg(target_arch = "wasm32")]
pub use opfs_storage::OpfsStorage;

/// Async OPFS blob store mirroring `lunco-precompute`'s `<namespace>/<key-hex>`
/// cache layout — the wasm counterpart of that crate's native-only sync fs
/// tier. Driven with `spawn_local` (non-`Send` futures; see [`opfs_storage`]).
#[cfg(target_arch = "wasm32")]
pub mod opfs_blob;

// ─────────────────────────────────────────────────────────────────────────────
// Errors
// ─────────────────────────────────────────────────────────────────────────────

/// Errors produced by [`Storage`] operations.
///
/// Uses an `enum` of semantic cases instead of a bag of strings so UI
/// code can branch on `ReadOnly` vs `NotFound` vs a generic I/O error
/// without regex-matching `Display` messages.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// Handle refers to something that doesn't exist.
    #[error("not found")]
    NotFound,

    /// Handle is read-only (MSL libraries, remote snapshots, etc.).
    #[error("handle is read-only")]
    ReadOnly,

    /// User dismissed a picker without choosing anything.
    #[error("cancelled")]
    Cancelled,

    /// Underlying I/O failure. Wraps the OS error for display only —
    /// caller should usually turn this into an "error" toast.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// The chosen backend doesn't support this handle kind. Example:
    /// asking `FileStorage::read` about an `Http` handle.
    #[error("unsupported: {0}")]
    Unsupported(String),
}

/// Result alias for storage operations.
pub type StorageResult<T> = Result<T, StorageError>;

// ─────────────────────────────────────────────────────────────────────────────
// Handle — opaque address into a storage backend
// ─────────────────────────────────────────────────────────────────────────────

/// An opaque address into a storage backend.
///
/// Variants exist for every backend we plan to support; today only
/// [`StorageHandle::File`] (native filesystem) and
/// [`StorageHandle::Memory`] (in-memory for tests) are implemented. The
/// other variants are defined now so match arms in higher-level code
/// stay exhaustive when we add the backends later — no downstream
/// patch required except handling the new case.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum StorageHandle {
    /// A path on the native filesystem.
    File(PathBuf),

    /// In-memory entry keyed by an arbitrary string (tests, transient
    /// untitled buffers that don't need a UUID).
    Memory(String),

    /// File-System-Access handle token (browser, Chromium-family).
    /// Opaque — the wasm backend unpacks this at call time. Token is
    /// a stable UUID the backend maps to a JS `FileSystemHandle`.
    #[cfg(any(feature = "fsa_stub", doc))]
    Fsa(String),

    /// IndexedDB entry: `db` is the database name, `key` the record id.
    #[cfg(any(feature = "idb_stub", doc))]
    Idb {
        /// Database name.
        db: String,
        /// Record key.
        key: String,
    },

    /// Origin Private File System path (browser local-first storage).
    #[cfg(any(feature = "opfs_stub", doc))]
    Opfs(String),

    /// Remote HTTPS endpoint. `PUT`/`GET` via whatever the backend
    /// does (can be LunCo's own API server or any compatible remote).
    #[cfg(any(feature = "http_stub", doc))]
    Http(String),
}

impl StorageHandle {
    /// Short display name — the last path segment for files, or the
    /// key for in-memory entries. Used for tab titles, error toasts,
    /// breadcrumbs.
    pub fn display_name(&self) -> String {
        match self {
            Self::File(p) => p
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("(invalid)")
                .to_string(),
            Self::Memory(k) => k.clone(),
            // Browser backends: show the last path/URL segment (or the opaque
            // token) — the same "leaf name" intent as the File arm.
            #[cfg(any(feature = "fsa_stub", doc))]
            Self::Fsa(token) => token.clone(),
            #[cfg(any(feature = "idb_stub", doc))]
            Self::Idb { key, .. } => key.clone(),
            #[cfg(any(feature = "opfs_stub", doc))]
            Self::Opfs(path) => path.rsplit('/').find(|s| !s.is_empty()).unwrap_or(path).to_string(),
            #[cfg(any(feature = "http_stub", doc))]
            Self::Http(url) => url.rsplit('/').find(|s| !s.is_empty()).unwrap_or(url).to_string(),
        }
    }

    /// Parent handle of a File — the enclosing directory. Returns `None`
    /// for root paths and non-File variants.
    pub fn parent(&self) -> Option<StorageHandle> {
        match self {
            Self::File(p) => p.parent().map(|parent| Self::File(parent.to_path_buf())),
            _ => None,
        }
    }

    /// Whether this handle's path lies under `root`. Used by
    /// `Twin::owns()` to decide whether a document belongs to a twin's
    /// folder without materialising a document list on the twin side.
    /// Only meaningful for [`StorageHandle::File`] pairs today — cross-
    /// backend comparisons always return `false`.
    pub fn is_under(&self, root: &StorageHandle) -> bool {
        match (self, root) {
            (Self::File(p), Self::File(r)) => path_is_under(p, r),
            _ => false,
        }
    }

    /// Borrowed filesystem path, if this is a [`StorageHandle::File`].
    /// Consumers that genuinely need a `Path` (compile pipeline feeding
    /// rumoca, for instance) use this rather than pattern-matching on
    /// the enum directly.
    pub fn as_file_path(&self) -> Option<&Path> {
        match self {
            Self::File(p) => Some(p.as_path()),
            _ => None,
        }
    }
}

fn path_is_under(p: &Path, root: &Path) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        match (p.canonicalize(), root.canonicalize()) {
            (Ok(pp), Ok(rp)) => pp.starts_with(rp),
            // Fall back to prefix-string comparison if either path can't be
            // canonicalised (e.g. referenced file was just deleted). Not
            // symlink-safe but the common case works.
            _ => p.starts_with(root),
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        p.starts_with(root)
    }
}

// Picker parameter types (`OpenFilter`/`SaveHint`) moved to the workbench's
// picker with the dialog itself — see the note on the `Storage` trait below.

/// Persist `bytes` to a file `path` **through the [`Storage`] API** (CQ-107).
///
/// A thin, cross-target path-convenience that routes through whichever backend
/// owns `StorageHandle::File` on this platform — so callers persisting a
/// config/session file get correct behaviour on both targets from one call,
/// and the storage backend stays the single I/O chokepoint instead of each
/// crate reaching for `std::fs::write` + a hand-rolled `rename`:
///
/// - **native** → [`FileStorage`]: `FileStorage::write`'s tmp+rename atomic
///   replace; a crash mid-write leaves the prior file intact, never truncated.
/// - **wasm** → [`WebStorage`]: maps the path onto a `localStorage` key.
///
/// (Before this was wasm-aware, a wasm caller that invoked it failed to
/// *compile* — the fn was `#[cfg(not(wasm32))]`-gated — which is why
/// `recents.rs` / `workspace_state.rs` broke the wasm build.)
#[cfg(not(target_arch = "wasm32"))]
pub fn write_file_sync(path: &Path, bytes: &[u8]) -> StorageResult<()> {
    FileStorage::new().write_sync(&StorageHandle::File(path.to_path_buf()), bytes)
}

/// Wasm counterpart of [`write_file_sync`] — see that fn's docs. Routes the
/// `File` handle through [`WebStorage`] (`localStorage`) so the same call site
/// persists on the web without a `#[cfg]` at every caller.
#[cfg(target_arch = "wasm32")]
pub fn write_file_sync(path: &Path, bytes: &[u8]) -> StorageResult<()> {
    WebStorage::new().write_sync(&StorageHandle::File(path.to_path_buf()), bytes)
}

/// Read a file `path` **through the [`Storage`] API** — the read counterpart of
/// [`write_file_sync`]. Routes to whichever backend owns `StorageHandle::File`
/// on this platform, so a caller loading a config/session file uses one call on
/// both targets (native [`FileStorage`] / wasm [`WebStorage`]). Returns
/// [`StorageError::NotFound`] when the file / localStorage key is absent.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_file_sync(path: &Path) -> StorageResult<Vec<u8>> {
    FileStorage::new().read_sync(&StorageHandle::File(path.to_path_buf()))
}

/// Wasm counterpart of [`read_file_sync`] — routes the `File` handle through
/// [`WebStorage`] (`localStorage`).
#[cfg(target_arch = "wasm32")]
pub fn read_file_sync(path: &Path) -> StorageResult<Vec<u8>> {
    WebStorage::new().read_sync(&StorageHandle::File(path.to_path_buf()))
}

// ─────────────────────────────────────────────────────────────────────────────
// The trait
// ─────────────────────────────────────────────────────────────────────────────

/// Abstraction over a storage backend.
///
/// # Sync vs async
///
/// Reads and writes are **synchronous** because the common cases (small
/// text files on native disk, in-memory blobs) complete in microseconds.
/// If a backend needs to go async (HTTP, IPFS), it does so behind the
/// trait by blocking on an internal runtime — the caller keeps the same
/// signature and wraps the call in [`bevy::tasks::AsyncComputeTaskPool`]
/// when the workload warrants.
///
/// Pickers are **asynchronous** today (native rfd dialogs block the
/// task thread, but the workbench observers poll them without blocking
/// the UI; on wasm they are truly async browser-side).
#[async_trait::async_trait]
pub trait Storage: Send + Sync {
    /// Read the full contents of a handle.
    async fn read(&self, handle: &StorageHandle) -> StorageResult<Vec<u8>>;

    /// Write bytes to a handle, replacing existing content atomically
    /// where the backend supports it. [`FileStorage`]'s `File` writes do:
    /// they tmp-write + `rename`, so a crash mid-write leaves the prior
    /// file intact, never a truncated one. For a path-based one-liner see
    /// [`write_file_sync`].
    async fn write(&self, handle: &StorageHandle, bytes: &[u8]) -> StorageResult<()>;

    /// Synchronous convenience wrapper around [`Storage::write`].
    ///
    /// Blocks the calling thread on the write future. Safe for backends
    /// whose futures resolve without yielding — notably [`FileStorage`],
    /// whose async fns wrap synchronous `std::fs`, so the future is
    /// already `Ready` and `block_on` returns immediately. Do **not**
    /// call this on a genuinely-async backend (HTTP, IndexedDB) from the
    /// main thread; `.await` or a task pool instead.
    ///
    /// Exists so callers in clippy-gated crates (which ban direct
    /// `std::fs`) can do a one-shot file write through the storage
    /// abstraction without standing up an async task pipeline.
    fn write_sync(&self, handle: &StorageHandle, bytes: &[u8]) -> StorageResult<()> {
        futures_lite::future::block_on(self.write(handle, bytes))
    }

    /// Synchronous convenience wrapper around [`Storage::read`]. Same
    /// caveats as [`Storage::write_sync`].
    fn read_sync(&self, handle: &StorageHandle) -> StorageResult<Vec<u8>> {
        futures_lite::future::block_on(self.read(handle))
    }

    /// Cheap "does this exist?" probe. Backends that can't implement
    /// it cheaply (e.g. always-fetch HTTP) should answer `false` on
    /// error rather than making a round-trip.
    async fn exists(&self, handle: &StorageHandle) -> bool;

    /// Whether this handle would reject a write (MSL library file,
    /// read-only FS mount, remote snapshot). Pure advisory — a final
    /// `write` is the ground truth.
    async fn is_writable(&self, handle: &StorageHandle) -> bool;

    // NOTE: file-OPEN/SAVE/FOLDER pickers are a UI concern and live in the
    // workbench (`lunco_workbench::picker`, native `rfd` + future wasm FSA), NOT
    // on this I/O trait. Keeping `rfd` out of `lunco-storage` keeps the crate (and
    // the 8 crates that depend on it, incl. the headless server) free of the
    // native file-dialog → wayland/winit pull.
}
