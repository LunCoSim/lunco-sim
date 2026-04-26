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

// ─────────────────────────────────────────────────────────────────────────────
// Picker parameters
// ─────────────────────────────────────────────────────────────────────────────

/// One entry in a picker's file-type filter list. A picker may show
/// several — e.g. "Modelica models", "All files".
#[derive(Debug, Clone)]
pub struct OpenFilter {
    /// Human-readable group label ("Modelica models").
    pub name: String,
    /// Extensions without the leading dot ("mo", "mos").
    pub extensions: Vec<String>,
}

impl OpenFilter {
    /// Convenience constructor.
    pub fn new(name: impl Into<String>, extensions: &[&str]) -> Self {
        Self {
            name: name.into(),
            extensions: extensions.iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// Hints for a save dialog: starting directory, default filename,
/// and filter list. All optional; the picker falls back to its own
/// defaults when a field is missing.
#[derive(Debug, Clone, Default)]
pub struct SaveHint {
    /// Default filename shown in the picker.
    pub suggested_name: Option<String>,
    /// Starting directory. For a new document previously saved, this
    /// is usually the document's own origin folder so "Save As" opens
    /// next to the existing file.
    pub start_dir: Option<StorageHandle>,
    /// File type filters offered in the picker.
    pub filters: Vec<OpenFilter>,
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
/// Pickers are **synchronous** today (native rfd dialogs block the
/// calling thread, which is expected — the user is looking at a modal
/// system dialog). The wasm backend will switch to async when we get
/// there; we'll revisit the trait at that point rather than paying
/// `async_trait` costs every call now.
pub trait Storage: Send + Sync {
    /// Read the full contents of a handle.
    fn read(&self, handle: &StorageHandle) -> StorageResult<Vec<u8>>;

    /// Write bytes to a handle, replacing existing content atomically
    /// where the backend supports it.
    fn write(&self, handle: &StorageHandle, bytes: &[u8]) -> StorageResult<()>;

    /// Cheap "does this exist?" probe. Backends that can't implement
    /// it cheaply (e.g. always-fetch HTTP) should answer `false` on
    /// error rather than making a round-trip.
    fn exists(&self, handle: &StorageHandle) -> bool;

    /// Whether this handle would reject a write (MSL library file,
    /// read-only FS mount, remote snapshot). Pure advisory — a final
    /// `write` is the ground truth.
    fn is_writable(&self, handle: &StorageHandle) -> bool;

    /// Show an "open" picker and return the chosen handle.
    /// Returns `Ok(None)` if the user cancelled.
    fn pick_open(&self, filter: &OpenFilter) -> StorageResult<Option<StorageHandle>>;

    /// Show a "save as" picker and return the chosen handle.
    /// Returns `Ok(None)` if the user cancelled.
    fn pick_save(&self, hint: &SaveHint) -> StorageResult<Option<StorageHandle>>;

    /// Show an "open folder" picker. Used for "Open Twin folder" and
    /// "Open Workspace folder" flows.
    fn pick_folder(&self) -> StorageResult<Option<StorageHandle>>;
}
