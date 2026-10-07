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

use std::path::{Path, PathBuf};

mod path_identity;
pub use path_identity::FilePathIdentity;

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

    /// A create-only write would replace an existing entry.
    #[error("entry already exists")]
    AlreadyExists,

    /// Contents exceed the caller's byte budget for this read.
    #[error("contents exceed the {max_bytes}-byte read limit")]
    SizeLimitExceeded { max_bytes: usize },

    /// Streamed contents exceed the caller's actual-byte budget (not metadata).
    #[error("contents exceed the {max_bytes}-byte streaming limit")]
    StreamingSizeLimitExceeded { max_bytes: u64 },

    /// Handle is read-only (source library libraries, remote snapshots, etc.).
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

/// Decode a standard `file:` URI into its native filesystem path.
///
/// `Ok(None)` means the spelling has no `file:` scheme: callers may then
/// route another asset scheme or keep a raw native path unchanged. An invalid
/// file URI is an error, never a raw-path fallback. Decoding, drive letters,
/// localhost, and Windows UNC authorities are owned by the `url` crate.
/// Browser builds reject file URIs because they have no native filesystem.
pub fn file_uri_to_path(reference: &str) -> StorageResult<Option<PathBuf>> {
    if !reference
        .split_once(':')
        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("file"))
    {
        return Ok(None);
    }
    let invalid = |detail: String| {
        StorageError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid file URI `{reference}`: {detail}"),
        ))
    };
    let violation = std::cell::Cell::new(None);
    let report_violation = |value| violation.set(Some(value));
    let uri = url::Url::options()
        .syntax_violation_callback(Some(&report_violation))
        .parse(reference)
        .map_err(|error| invalid(error.to_string()))?;
    if let Some(violation) = violation.get() {
        return Err(invalid(violation.to_string()));
    }
    if uri.query().is_some() || uri.fragment().is_some() {
        return Err(invalid(
            "filesystem paths cannot contain a URI query or fragment".into(),
        ));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let path = uri
            .to_file_path()
            .map_err(|()| invalid("path or authority is not valid on this platform".into()))?;
        if path.as_os_str().as_encoded_bytes().contains(&0) {
            return Err(invalid("filesystem paths cannot contain NUL bytes".into()));
        }
        Ok(Some(path))
    }
    #[cfg(target_arch = "wasm32")]
    {
        Err(StorageError::Unsupported(
            "file URIs require a native filesystem".into(),
        ))
    }
}

/// Convert a USD-authored Windows drive/UNC reference to the native file contract.
///
/// Logical `/...` asset roots retain their meaning. Drive-relative addresses
/// are rejected because their interpretation depends on per-drive process state.
/// Foreign Windows filesystem addresses fail explicitly on non-Windows hosts.
pub fn windows_file_reference_uri(reference: &str) -> StorageResult<Option<String>> {
    let bytes = reference.as_bytes();
    let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    let unc = reference.starts_with(r"\\") || reference.starts_with("//");
    if !drive && !unc {
        return Ok(None);
    }
    #[cfg(windows)]
    {
        let path = Path::new(reference);
        if !path.is_absolute() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("USD filesystem reference `{reference}` must be fully qualified"),
            )
            .into());
        }
        file_path_to_uri(path).map(Some)
    }
    #[cfg(not(windows))]
    {
        Err(StorageError::Unsupported(format!(
            "Windows filesystem reference `{reference}` requires a Windows host"
        )))
    }
}

/// Encode an absolute native filesystem path as a standard `file:` URI.
/// Relative paths and browser storage keys are not native file URI addresses.
pub fn file_path_to_uri(path: &Path) -> StorageResult<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if path.as_os_str().as_encoded_bytes().contains(&0) {
            return Err(StorageError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "filesystem paths cannot contain NUL bytes",
            )));
        }
        url::Url::from_file_path(path)
            .map(|uri| uri.into())
            .map_err(|()| {
                StorageError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("cannot encode native path {} as a file URI", path.display()),
                ))
            })
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
        Err(StorageError::Unsupported(
            "file URIs require a native filesystem".into(),
        ))
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod file_uri_tests {
    use super::*;

    #[test]
    fn authored_windows_references_use_native_identity_or_explicit_host_rejection() {
        for reference in [
            r"C:\My Twin\part#%.usda",
            "C:/My Twin/part#%.usda",
            r"\\server\share\My Twin\part.usda",
            r"\\?\C:\My Twin\part.usda",
        ] {
            #[cfg(windows)]
            {
                let uri = windows_file_reference_uri(reference).unwrap().unwrap();
                let decoded = file_uri_to_path(&uri).unwrap().unwrap();
                assert_eq!(file_path_to_uri(&decoded).unwrap(), uri);
            }
            #[cfg(not(windows))]
            assert!(windows_file_reference_uri(reference).is_err());
        }
        assert!(windows_file_reference_uri("C:part.usda").is_err());
        for reference in [
            "lunco://models/part.usda",
            "twin://root/part.usda",
            "/models/part.usda",
            "./part#%.usda",
        ] {
            assert_eq!(windows_file_reference_uri(reference).unwrap(), None);
        }
    }

    #[test]
    fn file_uris_preserve_native_paths_and_reject_invalid_addresses() {
        let path = std::env::temp_dir().join("провајдер with spaces.mo");
        let uri = file_path_to_uri(&path).unwrap();
        assert!(uri.contains("%20"));
        assert_eq!(file_uri_to_path(&uri).unwrap(), Some(path.clone()));
        let localhost = uri.replacen("file://", "file://localhost", 1);
        assert_eq!(file_uri_to_path(&localhost).unwrap(), Some(path));
        for native_or_asset in [
            r"C:\Projects\My Twin\scene.usda",
            r"\\server\share\scene.usda",
            "relative/with spaces.mo",
            "twin://demo/scene.usda",
        ] {
            assert_eq!(file_uri_to_path(native_or_asset).unwrap(), None);
        }
        for invalid in [
            "file:///invalid%00.mo",
            "file:///invalid%GG.mo",
            "file:///model.mo?version=2",
            "file:///model.mo#Main",
            r"file://C:\Projects\model.mo",
            "file://user@localhost/model.mo",
        ] {
            assert!(
                file_uri_to_path(invalid).is_err(),
                "{invalid:?} must be rejected"
            );
        }
        assert!(file_path_to_uri(Path::new("relative.mo")).is_err());

        #[cfg(windows)]
        {
            assert_eq!(
                file_uri_to_path("FILE:///C:/Projects/My%20Twin/scene.usda").unwrap(),
                Some(PathBuf::from(r"C:\Projects\My Twin\scene.usda"))
            );
            assert_eq!(
                file_uri_to_path("file://server/share/My%20Twin/scene.usda").unwrap(),
                Some(PathBuf::from(r"\\server\share\My Twin\scene.usda"))
            );
        }
        #[cfg(unix)]
        {
            assert_eq!(
                file_uri_to_path("FILE:///home/user/My%20Twin/scene.usda").unwrap(),
                Some(PathBuf::from("/home/user/My Twin/scene.usda"))
            );
            assert!(file_uri_to_path("file://server/share/scene.usda").is_err());
        }
    }
}

/// The kind of entry addressed by a storage handle.
///
/// Directory identity is part of the storage backend rather than a caller
/// probing a native path. Backends without directory semantics must report
/// [`StorageError::Unsupported`] instead of guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageEntryKind {
    /// A regular file/blob entry.
    File,
    /// A directory/container entry.
    Directory,
    /// A symbolic link itself, including a broken link. Emitted only by native
    /// no-follow inspection; followed `Storage::entry_kind` and OPFS never emit it.
    Symlink,
}

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
            Self::Opfs(path) => path
                .rsplit('/')
                .find(|s| !s.is_empty())
                .unwrap_or(path)
                .to_string(),
            #[cfg(any(feature = "http_stub", doc))]
            Self::Http(url) => url
                .rsplit('/')
                .find(|s| !s.is_empty())
                .unwrap_or(url)
                .to_string(),
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
            // Ownership is an authority decision, so an unresolved path is
            // not close enough. A lexical fallback can claim a deleted file
            // or an unresolved symlink for the wrong Twin; wait for a
            // canonical path instead.
            _ => false,
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

/// Read UTF-8 text from a file through the [`Storage`] API.
pub fn read_text_file_sync(path: &Path) -> StorageResult<String> {
    String::from_utf8(read_file_sync(path)?).map_err(|error| {
        StorageError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })
}

/// Delete a file `path` **through the [`Storage`] API** — the delete
/// counterpart of [`write_file_sync`]. Routes to whichever backend owns
/// `StorageHandle::File` on this platform (native [`FileStorage`] / wasm
/// [`WebStorage`]). Returns [`StorageError::NotFound`] when the file /
/// localStorage key is already absent.
#[cfg(not(target_arch = "wasm32"))]
pub fn delete_file_sync(path: &Path) -> StorageResult<()> {
    FileStorage::new().delete_sync(&StorageHandle::File(path.to_path_buf()))
}

/// Wasm counterpart of [`delete_file_sync`] — routes the `File` handle through
/// [`WebStorage`] (`localStorage`).
#[cfg(target_arch = "wasm32")]
pub fn delete_file_sync(path: &Path) -> StorageResult<()> {
    WebStorage::new().delete_sync(&StorageHandle::File(path.to_path_buf()))
}

/// Resolve a native file path through the storage boundary.
///
/// This is used for path identity/security checks that must agree with the
/// native backend used for the subsequent read or write. The browser counterpart
/// validates the actual OPFS logical identity.
#[cfg(not(target_arch = "wasm32"))]
pub fn canonicalize_file_path(path: &Path) -> StorageResult<PathBuf> {
    std::fs::canonicalize(path).map_err(StorageError::Io)
}

/// Canonical OPFS identity. Absolute-root markers address the same private
/// tree as relative paths; parent traversal and non-UTF-8 names are rejected.
#[cfg(target_arch = "wasm32")]
pub fn canonicalize_file_path(path: &Path) -> StorageResult<PathBuf> {
    opfs_file_identity(path)
}

#[cfg(any(target_arch = "wasm32", test))]
fn opfs_file_identity(path: &Path) -> StorageResult<PathBuf> {
    let mut canonical = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(name) => {
                let name = name
                    .to_str()
                    .filter(|name| !name.chars().any(char::is_control))
                    .ok_or_else(|| {
                        StorageError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "invalid OPFS path component",
                        ))
                    })?;
                canonical.push(name);
            }
            std::path::Component::RootDir | std::path::Component::CurDir => {}
            _ => {
                return Err(StorageError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "OPFS parent traversal or native prefix is invalid",
                )));
            }
        }
    }
    if canonical.as_os_str().is_empty() {
        return Err(StorageError::NotFound);
    }
    Ok(canonical)
}

/// Ensure a directory exists through the active storage backend.
#[cfg(not(target_arch = "wasm32"))]
pub fn ensure_directory_sync(path: &Path) -> StorageResult<()> {
    FileStorage::new().ensure_directory_sync(&StorageHandle::File(path.to_path_buf()))
}

/// Browser counterpart of [`ensure_directory_sync`]. Browser storage has no
/// native directory tree; writes create their logical keys directly.
#[cfg(target_arch = "wasm32")]
pub fn ensure_directory_sync(_path: &Path) -> StorageResult<()> {
    Ok(())
}

/// Create a file symlink through the native storage boundary.
///
/// Symlinks are a native filesystem security concern, so higher-level tests
/// and path-owning crates use this helper instead of importing an OS-specific
/// filesystem module themselves. Browser storage has no symlink primitive and
/// therefore does not expose this operation.
#[cfg(all(not(target_arch = "wasm32"), unix))]
pub fn create_file_symlink_sync(target: &Path, link: &Path) -> StorageResult<()> {
    std::os::unix::fs::symlink(target, link).map_err(StorageError::Io)
}

/// Create a directory symlink through the native storage boundary.
///
/// See [`create_file_symlink_sync`] for why this operation belongs here.
#[cfg(all(not(target_arch = "wasm32"), unix))]
pub fn create_directory_symlink_sync(target: &Path, link: &Path) -> StorageResult<()> {
    std::os::unix::fs::symlink(target, link).map_err(StorageError::Io)
}

/// Create a file symlink through the native storage boundary on Windows.
#[cfg(windows)]
pub fn create_file_symlink_sync(target: &Path, link: &Path) -> StorageResult<()> {
    std::os::windows::fs::symlink_file(target, link).map_err(StorageError::Io)
}

/// Create a directory symlink through the native storage boundary on Windows.
#[cfg(windows)]
pub fn create_directory_symlink_sync(target: &Path, link: &Path) -> StorageResult<()> {
    std::os::windows::fs::symlink_dir(target, link).map_err(StorageError::Io)
}

/// List the direct children of a directory through the active storage
/// backend. The returned paths are sorted to make source scanners and tests
/// deterministic.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_directory_sync(path: &Path) -> StorageResult<Vec<PathBuf>> {
    FileStorage::new()
        .read_directory_sync(&StorageHandle::File(path.to_path_buf()))
        .map(|entries| {
            entries
                .into_iter()
                .filter_map(|handle| handle.as_file_path().map(Path::to_path_buf))
                .collect()
        })
}

/// Browser counterpart of [`read_directory_sync`]. Browser local storage has
/// no native directory tree, so callers must use a backend with directory
/// semantics when one is available.
#[cfg(target_arch = "wasm32")]
pub fn read_directory_sync(_path: &Path) -> StorageResult<Vec<PathBuf>> {
    Err(StorageError::Unsupported(
        "WebStorage has no directory listing".into(),
    ))
}

/// Identify a path through the platform's default file backend.
#[cfg(not(target_arch = "wasm32"))]
pub fn entry_kind_file_sync(path: &Path) -> StorageResult<StorageEntryKind> {
    FileStorage::new().entry_kind_sync(&StorageHandle::File(path.to_path_buf()))
}

/// Identify the native entry being moved without dereferencing its final link.
/// Intermediate parent resolution remains the caller's ownership preflight.
#[cfg(not(target_arch = "wasm32"))]
pub fn entry_kind_no_follow_file_sync(path: &Path) -> StorageResult<StorageEntryKind> {
    FileStorage::new().entry_kind_no_follow(path)
}

/// Browser entries have no symbolic links, so identifying the entry itself
/// uses the existing default browser file backend's entry-kind contract.
#[cfg(target_arch = "wasm32")]
pub fn entry_kind_no_follow_file_sync(path: &Path) -> StorageResult<StorageEntryKind> {
    entry_kind_file_sync(path)
}

/// Wasm counterpart of [`entry_kind_file_sync`].
#[cfg(target_arch = "wasm32")]
pub fn entry_kind_file_sync(path: &Path) -> StorageResult<StorageEntryKind> {
    WebStorage::new().entry_kind_sync(&StorageHandle::File(path.to_path_buf()))
}

/// Move a path through the platform's default file backend.
#[cfg(not(target_arch = "wasm32"))]
pub fn rename_file_sync(from: &Path, to: &Path) -> StorageResult<()> {
    FileStorage::new().rename_sync(
        &StorageHandle::File(from.to_path_buf()),
        &StorageHandle::File(to.to_path_buf()),
    )
}

/// Wasm counterpart of [`rename_file_sync`].
#[cfg(target_arch = "wasm32")]
pub fn rename_file_sync(from: &Path, to: &Path) -> StorageResult<()> {
    WebStorage::new().rename_sync(
        &StorageHandle::File(from.to_path_buf()),
        &StorageHandle::File(to.to_path_buf()),
    )
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

    /// Create an entry only when it does not already exist.
    ///
    /// Backends should implement this atomically when their storage contract
    /// supports it. The default checks the entry then writes; native
    /// [`FileStorage`] uses an atomic no-replace commit for file handles.
    async fn write_new(&self, handle: &StorageHandle, bytes: &[u8]) -> StorageResult<()> {
        match self.entry_kind(handle).await {
            Ok(_) => Err(StorageError::AlreadyExists),
            Err(StorageError::NotFound) => self.write(handle, bytes).await,
            Err(error) => Err(error),
        }
    }

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

    /// Synchronous convenience wrapper around [`Storage::write_new`].
    fn write_new_sync(&self, handle: &StorageHandle, bytes: &[u8]) -> StorageResult<()> {
        futures_lite::future::block_on(self.write_new(handle, bytes))
    }

    /// Synchronous convenience wrapper around [`Storage::read`]. Same
    /// caveats as [`Storage::write_sync`].
    fn read_sync(&self, handle: &StorageHandle) -> StorageResult<Vec<u8>> {
        futures_lite::future::block_on(self.read(handle))
    }

    /// Delete the entry addressed by `handle`. Returns
    /// [`StorageError::NotFound`] when there is nothing to delete, so a
    /// caller can distinguish "already gone" from a real failure. For a
    /// path-based one-liner see [`delete_file_sync`].
    async fn delete(&self, handle: &StorageHandle) -> StorageResult<()>;

    /// Synchronous convenience wrapper around [`Storage::delete`]. Same
    /// caveats as [`Storage::write_sync`].
    fn delete_sync(&self, handle: &StorageHandle) -> StorageResult<()> {
        futures_lite::future::block_on(self.delete(handle))
    }

    /// Identify the entry addressed by `handle`, following native symbolic links.
    async fn entry_kind(&self, handle: &StorageHandle) -> StorageResult<StorageEntryKind>;

    /// Synchronous convenience wrapper around [`Storage::entry_kind`].
    fn entry_kind_sync(&self, handle: &StorageHandle) -> StorageResult<StorageEntryKind> {
        futures_lite::future::block_on(self.entry_kind(handle))
    }

    /// Ensure a directory/container exists for a handle.
    ///
    /// Backends without directory semantics may return `Ok(())`; writes to
    /// their logical keys do not need a physical parent directory.
    async fn ensure_directory(&self, handle: &StorageHandle) -> StorageResult<()>;

    /// Synchronous convenience wrapper around [`Storage::ensure_directory`].
    fn ensure_directory_sync(&self, handle: &StorageHandle) -> StorageResult<()> {
        futures_lite::future::block_on(self.ensure_directory(handle))
    }

    /// List the direct children of a directory.
    ///
    /// Backends without directory semantics must return
    /// [`StorageError::Unsupported`] rather than fabricating an empty list.
    async fn read_directory(&self, handle: &StorageHandle) -> StorageResult<Vec<StorageHandle>>;

    /// Synchronous convenience wrapper around [`Storage::read_directory`].
    fn read_directory_sync(&self, handle: &StorageHandle) -> StorageResult<Vec<StorageHandle>> {
        futures_lite::future::block_on(self.read_directory(handle))
    }

    /// Move an entry from one handle to another without changing its bytes.
    /// Backends that do not support a native move return
    /// [`StorageError::Unsupported`] rather than emulating it with a
    /// read/write/delete sequence.
    async fn rename(&self, from: &StorageHandle, to: &StorageHandle) -> StorageResult<()>;

    /// Synchronous convenience wrapper around [`Storage::rename`].
    fn rename_sync(&self, from: &StorageHandle, to: &StorageHandle) -> StorageResult<()> {
        futures_lite::future::block_on(self.rename(from, to))
    }

    /// Cheap "does this exist?" probe. Backends that can't implement
    /// it cheaply (e.g. always-fetch HTTP) should answer `false` on
    /// error rather than making a round-trip.
    async fn exists(&self, handle: &StorageHandle) -> bool;

    /// Whether this handle would reject a write (source-library file,
    /// read-only FS mount, remote snapshot). Pure advisory — a final
    /// `write` is the ground truth.
    async fn is_writable(&self, handle: &StorageHandle) -> bool;

    // NOTE: file-OPEN/SAVE/FOLDER pickers are a UI concern and live in the
    // workbench file-dialog capability (`lunco_workbench_file_dialog`, native `rfd` + wasm), NOT
    // on this I/O trait. Keeping `rfd` out of `lunco-storage` keeps the crate (and
    // the 8 crates that depend on it, incl. the headless server) free of the
    // native file-dialog → wayland/winit pull.
}

#[cfg(test)]
mod opfs_identity_tests {
    #[test]
    fn mounted_opfs_identity_rejects_traversal_and_matches_storage_names() {
        use std::path::{Path, PathBuf};
        assert_eq!(
            super::opfs_file_identity(Path::new("/cache/twin # % Мир/model.mo")).unwrap(),
            PathBuf::from("cache/twin # % Мир/model.mo")
        );
        assert!(super::opfs_file_identity(Path::new("cache/a/../b/model.mo")).is_err());
        assert!(super::opfs_file_identity(Path::new("cache/invalid\0.mo")).is_err());
        assert!(super::opfs_file_identity(Path::new("/")).is_err());
    }
}

/// Deterministic bounded regular-file snapshot; enumeration never retains more
/// than the caller's cap, even when the backing directory contains many files.
#[derive(Debug)]
pub struct BoundedDirectoryEntries {
    pub entries: Vec<StorageHandle>,
    pub truncated: bool,
}
impl BoundedDirectoryEntries {
    pub(crate) fn retain_entry(&mut self, entry: StorageHandle, cap: usize) {
        let name = entry.display_name();
        let index = self
            .entries
            .partition_point(|current| current.display_name() < name);
        if self.entries.len() == cap {
            self.truncated = true;
            if index >= cap {
                return;
            }
            self.entries.pop();
        }
        self.entries.insert(index, entry);
    }
}
