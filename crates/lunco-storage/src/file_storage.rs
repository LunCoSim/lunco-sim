//! Native filesystem backend for [`crate::Storage`].
//!
//! Reads / writes via `std::fs`. Only handles [`StorageHandle::File`] and
//! [`StorageHandle::Memory`] variants — other variants return
//! [`StorageError::Unsupported`]. (File-open/save pickers are a UI concern and
//! live in `lunco_workbench_file_dialog`, not on the `Storage` trait.)
//!
//! `Memory` is included here so unit / integration tests don't need a
//! real temp dir. A single in-process map stores the blobs; different
//! `FileStorage` instances do NOT share memory unless the app wires
//! them to — this matches the principle of least surprise for tests
//! (each test gets its own instance).
//!
//! `_ =>` arms in the match blocks below are forward-compatible
//! landings for variants introduced under features (Idb, Opfs, Fsa,
//! Http). When the default feature set doesn't include those variants
//! the arms are unreachable — silence the warning file-wide rather
//! than splitting every match by cfg.
#![allow(unreachable_patterns)]
// This crate *owns* local-fs persistence (native only) and is on the
// clippy.toml `disallowed_methods` allow-list (see workspace `clippy.toml`
// header). The `std::fs` calls below are all `#[cfg(not(wasm32))]`-guarded;
// on wasm the `WebStorage` backend in `web_storage.rs` is used instead.
#![cfg_attr(not(target_arch = "wasm32"), allow(clippy::disallowed_methods))]

use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::io::Read;
use std::sync::Mutex;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{Storage, StorageEntryKind, StorageError, StorageHandle, StorageResult};

/// Both native inspection contracts share metadata classification and errors;
/// callers choose whether the final entry or its target is the subject.
#[cfg(not(target_arch = "wasm32"))]
fn native_entry_kind(
    path: &std::path::Path,
    follow_links: bool,
) -> StorageResult<StorageEntryKind> {
    let metadata = if follow_links {
        std::fs::metadata(path)
    } else {
        std::fs::symlink_metadata(path)
    }
    .map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            StorageError::NotFound
        } else {
            StorageError::Io(error)
        }
    })?;
    Ok(if metadata.file_type().is_symlink() {
        StorageEntryKind::Symlink
    } else if metadata.is_dir() {
        StorageEntryKind::Directory
    } else {
        StorageEntryKind::File
    })
}

/// Native-filesystem backend.
///
/// Stateless for `File` operations (delegates to `std::fs`). The
/// `Memory` map is per-instance so tests can't accidentally leak state
/// into each other.
#[derive(Default)]
pub struct FileStorage {
    memory: Mutex<HashMap<String, Vec<u8>>>,
}

impl FileStorage {
    /// Native no-follow entry identity, used when moving an entry rather than
    /// reading its target. Broken symbolic links remain existing entries.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn entry_kind_no_follow(&self, path: &std::path::Path) -> StorageResult<StorageEntryKind> {
        native_entry_kind(path, false)
    }
}

/// Cross-process native cache publication transaction. A separate OS file
/// handle owns the exclusive lock until this guard drops. The persistent lock
/// file must never be removed or replaced while cache writers can use it.
#[cfg(not(target_arch = "wasm32"))]
pub struct DirectoryCacheTransaction {
    directory: std::path::PathBuf,
    _lock: std::fs::File,
}

/// One direct regular file. Symlinks and directories are excluded without
/// following them, and no directory-wide collection is allocated.
#[cfg(not(target_arch = "wasm32"))]
pub struct DirectoryCacheFile {
    pub handle: StorageHandle,
    pub bytes: u64,
}

#[cfg(not(target_arch = "wasm32"))]
impl DirectoryCacheTransaction {
    /// Canonical directory identity shared by aliases of the same cache root.
    pub fn directory(&self) -> &std::path::Path {
        &self.directory
    }

    /// Stream regular-file metadata while retaining the transaction borrow.
    /// Drop the iterator before deleting entries; enumeration order is unspecified.
    pub fn files(
        &self,
    ) -> StorageResult<impl Iterator<Item = StorageResult<DirectoryCacheFile>> + '_> {
        Ok(std::fs::read_dir(&self.directory)?.filter_map(|entry| {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => return Some(Err(StorageError::Io(error))),
            };
            // DirEntry::metadata does not traverse a symlink on Unix or Windows.
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(error) => return Some(Err(StorageError::Io(error))),
            };
            metadata.is_file().then(|| {
                Ok(DirectoryCacheFile {
                    handle: StorageHandle::File(entry.path()),
                    bytes: metadata.len(),
                })
            })
        }))
    }
}

impl FileStorage {
    /// Construct a fresh backend.
    pub fn new() -> Self {
        Self::default()
    }

    /// Serialize cooperating native cache writers across threads and processes.
    /// Call only on a worker thread, after heavy encoding/preparation completes.
    /// Publication, bounded enumeration and deletion use the existing Storage API.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn lock_cache_directory(
        &self,
        handle: &StorageHandle,
    ) -> StorageResult<DirectoryCacheTransaction> {
        let StorageHandle::File(directory) = handle else {
            return Err(StorageError::Unsupported(
                "native cache transactions require a File directory".into(),
            ));
        };
        std::fs::create_dir_all(directory)?;
        let directory = std::fs::canonicalize(directory)?;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join(".cache-lock"))?;
        // A separate open per transaction avoids reentrant/cloned-handle lock
        // semantics. Read+write is supported by both flock and Windows LockFileEx.
        lock.lock()?;
        Ok(DirectoryCacheTransaction {
            directory,
            _lock: lock,
        })
    }

    /// Read at most the caller's byte budget, rejecting oversized contents
    /// before returning bytes. File reads remain bounded if the file grows.
    pub async fn read_bounded(
        &self,
        handle: &StorageHandle,
        max_bytes: usize,
    ) -> StorageResult<Vec<u8>> {
        self.read_contents(handle, Some(max_bytes))
    }

    fn read_contents(
        &self,
        handle: &StorageHandle,
        max_bytes: Option<usize>,
    ) -> StorageResult<Vec<u8>> {
        match handle {
            #[cfg(not(target_arch = "wasm32"))]
            StorageHandle::File(path) => {
                let file = std::fs::File::open(path).map_err(|error| {
                    if error.kind() == std::io::ErrorKind::NotFound {
                        StorageError::NotFound
                    } else {
                        StorageError::Io(error)
                    }
                })?;
                read_contents(file, max_bytes)
            }
            StorageHandle::Memory(key) => {
                let map = self
                    .memory
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let bytes = map.get(key).ok_or(StorageError::NotFound)?;
                if let Some(max_bytes) = max_bytes
                    && bytes.len() > max_bytes
                {
                    return Err(StorageError::SizeLimitExceeded { max_bytes });
                }
                Ok(bytes.clone())
            }
            _ => Err(StorageError::Unsupported(
                "FileStorage does not handle web / remote variants".into(),
            )),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn read_contents(
    mut reader: impl std::io::Read,
    max_bytes: Option<usize>,
) -> StorageResult<Vec<u8>> {
    let mut bytes = Vec::new();
    if let Some(max_bytes) = max_bytes {
        // The sentinel distinguishes an exact-limit file from oversized data.
        // Take bounds actual I/O, independently of metadata or later growth.
        let read_limit = u64::try_from(max_bytes)
            .map_err(|_| {
                StorageError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "read budget exceeds the reader address space",
                ))
            })?
            .saturating_add(1);
        reader.take(read_limit).read_to_end(&mut bytes)?;
        if bytes.len() > max_bytes {
            return Err(StorageError::SizeLimitExceeded { max_bytes });
        }
    } else {
        reader.read_to_end(&mut bytes)?;
    }
    Ok(bytes)
}

#[cfg(not(target_arch = "wasm32"))]
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Atomic file replace (tmp + `fsync` + `rename`) — the implementation
/// behind the `File` arm of [`FileStorage::write`] (CQ-107). Private: the
/// world reaches this through the [`Storage`] API (`write` / `write_sync`),
/// never as a bare-path bypass, so the backend abstraction holds. A crash
/// mid-write leaves the prior file intact, never a truncated one. The temp
/// is a hidden per-process sibling so the rename stays within one
/// filesystem and won't collide with a concurrent writer's temp.
#[cfg(not(target_arch = "wasm32"))]
fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = stage_atomic_write(path, bytes)?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

/// Write a new file completely before linking it into place. `hard_link`
/// commits atomically and fails instead of replacing an existing destination.
#[cfg(not(target_arch = "wasm32"))]
fn atomic_write_new(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = stage_atomic_write(path, bytes)?;
    let commit = std::fs::hard_link(&temporary, path);
    let cleanup = std::fs::remove_file(&temporary);
    commit?;
    cleanup?;
    Ok(())
}

/// Reserve one unique sibling per write and finish its bytes before publishing.
/// Both replacement and create-only commits share this staging boundary.
#[cfg(not(target_arch = "wasm32"))]
fn stage_atomic_write(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<std::path::PathBuf> {
    use std::io::Write as _;
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("tmp");
    for _ in 0..32 {
        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary = path.with_file_name(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            sequence
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(mut file) => {
                let staged_result = file.write_all(bytes).and_then(|()| file.sync_all());
                drop(file);
                if let Err(error) = staged_result {
                    let _ = std::fs::remove_file(&temporary);
                    return Err(error);
                }
                return Ok(temporary);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "unable to reserve a unique storage staging file",
    ))
}

#[async_trait::async_trait]
impl Storage for FileStorage {
    async fn read(&self, handle: &StorageHandle) -> StorageResult<Vec<u8>> {
        self.read_contents(handle, None)
    }

    async fn write(&self, handle: &StorageHandle, bytes: &[u8]) -> StorageResult<()> {
        match handle {
            #[cfg(not(target_arch = "wasm32"))]
            StorageHandle::File(path) => {
                // CQ-107: honor the trait's documented atomic-replace
                // contract — tmp+rename (also creates parent dirs) instead
                // of a truncating `std::fs::write` that leaves a zero-byte
                // file if the process dies mid-write.
                atomic_write(path, bytes)?;
                Ok(())
            }
            StorageHandle::Memory(key) => {
                let mut map = self
                    .memory
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                map.insert(key.clone(), bytes.to_vec());
                Ok(())
            }
            _ => Err(StorageError::Unsupported(
                "FileStorage does not handle web / remote variants".into(),
            )),
        }
    }

    async fn write_new(&self, handle: &StorageHandle, bytes: &[u8]) -> StorageResult<()> {
        match handle {
            #[cfg(not(target_arch = "wasm32"))]
            StorageHandle::File(path) => atomic_write_new(path, bytes).map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    StorageError::AlreadyExists
                } else {
                    StorageError::Io(error)
                }
            }),
            StorageHandle::Memory(key) => {
                let mut map = self
                    .memory
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if map.contains_key(key) {
                    Err(StorageError::AlreadyExists)
                } else {
                    map.insert(key.clone(), bytes.to_vec());
                    Ok(())
                }
            }
            _ => Err(StorageError::Unsupported(
                "FileStorage does not handle web / remote variants".into(),
            )),
        }
    }

    async fn delete(&self, handle: &StorageHandle) -> StorageResult<()> {
        match handle {
            #[cfg(not(target_arch = "wasm32"))]
            StorageHandle::File(path) => match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(StorageError::NotFound),
                Err(e) => Err(StorageError::Io(e)),
            },
            StorageHandle::Memory(key) => {
                let mut map = self
                    .memory
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                map.remove(key).map(|_| ()).ok_or(StorageError::NotFound)
            }
            _ => Err(StorageError::Unsupported(
                "FileStorage does not handle web / remote variants".into(),
            )),
        }
    }

    async fn entry_kind(&self, handle: &StorageHandle) -> StorageResult<StorageEntryKind> {
        match handle {
            #[cfg(not(target_arch = "wasm32"))]
            StorageHandle::File(path) => native_entry_kind(path, true),
            StorageHandle::Memory(key) => {
                let map = self
                    .memory
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if map.contains_key(key) {
                    Ok(StorageEntryKind::File)
                } else {
                    Err(StorageError::NotFound)
                }
            }
            _ => Err(StorageError::Unsupported(
                "FileStorage does not identify web / remote variants".into(),
            )),
        }
    }

    async fn ensure_directory(&self, handle: &StorageHandle) -> StorageResult<()> {
        match handle {
            #[cfg(not(target_arch = "wasm32"))]
            StorageHandle::File(path) => std::fs::create_dir_all(path).map_err(StorageError::Io),
            StorageHandle::Memory(_) => Err(StorageError::Unsupported(
                "Memory handles have no directory container".into(),
            )),
            _ => Err(StorageError::Unsupported(
                "FileStorage does not create web / remote directories".into(),
            )),
        }
    }

    async fn read_directory(&self, handle: &StorageHandle) -> StorageResult<Vec<StorageHandle>> {
        match handle {
            #[cfg(not(target_arch = "wasm32"))]
            StorageHandle::File(path) => {
                let mut entries = std::fs::read_dir(path)
                    .map_err(|error| match error.kind() {
                        std::io::ErrorKind::NotFound => StorageError::NotFound,
                        _ => StorageError::Io(error),
                    })?
                    .map(|entry| entry.map(|entry| StorageHandle::File(entry.path())))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(StorageError::Io)?;
                entries.sort_by_key(|entry| entry.display_name());
                Ok(entries)
            }
            _ => Err(StorageError::Unsupported(
                "FileStorage does not list web / remote directories".into(),
            )),
        }
    }

    async fn rename(&self, from: &StorageHandle, to: &StorageHandle) -> StorageResult<()> {
        match (from, to) {
            #[cfg(not(target_arch = "wasm32"))]
            (StorageHandle::File(from), StorageHandle::File(to)) => {
                match std::fs::rename(from, to) {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        Err(StorageError::NotFound)
                    }
                    Err(e) => Err(StorageError::Io(e)),
                }
            }
            (StorageHandle::Memory(from), StorageHandle::Memory(to)) => {
                let mut map = self
                    .memory
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let Some(bytes) = map.remove(from) else {
                    return Err(StorageError::NotFound);
                };
                map.insert(to.clone(), bytes);
                Ok(())
            }
            _ => Err(StorageError::Unsupported(
                "FileStorage cannot rename between these handle kinds".into(),
            )),
        }
    }

    async fn exists(&self, handle: &StorageHandle) -> bool {
        match handle {
            #[cfg(not(target_arch = "wasm32"))]
            StorageHandle::File(path) => path.exists(),
            StorageHandle::Memory(key) => self
                .memory
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key(key),
            _ => false,
        }
    }

    async fn is_writable(&self, handle: &StorageHandle) -> bool {
        match handle {
            #[cfg(not(target_arch = "wasm32"))]
            StorageHandle::File(path) => {
                // If the file exists, consult its permissions; if it
                // doesn't, fall back to parent-dir writability so
                // "Save As into fresh path" returns `true`.
                if let Ok(meta) = std::fs::metadata(path) {
                    !meta.permissions().readonly()
                } else if let Some(parent) = path.parent() {
                    std::fs::metadata(parent)
                        .map(|m| !m.permissions().readonly())
                        .unwrap_or(true)
                } else {
                    true
                }
            }
            StorageHandle::Memory(_) => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_lite::future::block_on;
    use tempfile::tempdir;

    #[test]
    fn memory_roundtrip() {
        block_on(async {
            let s = FileStorage::new();
            let h = StorageHandle::Memory("k".into());
            assert!(!s.exists(&h).await);
            s.write(&h, b"hello").await.unwrap();
            assert!(s.exists(&h).await);
            assert_eq!(s.read(&h).await.unwrap(), b"hello");
            s.write(&h, b"world").await.unwrap();
            assert_eq!(s.read(&h).await.unwrap(), b"world");
        });
    }

    #[test]
    fn bounded_memory_reads_reject_oversized_contents_and_accept_boundaries() {
        block_on(async {
            let storage = FileStorage::new();
            let handle = StorageHandle::Memory("bounded".into());
            assert!(matches!(
                storage.read_bounded(&handle, 0).await,
                Err(StorageError::NotFound)
            ));
            storage.write(&handle, b"abc").await.unwrap();
            assert_eq!(storage.read_bounded(&handle, 3).await.unwrap(), b"abc");
            assert!(matches!(
                storage.read_bounded(&handle, 2).await,
                Err(StorageError::SizeLimitExceeded { max_bytes: 2 })
            ));
            assert!(matches!(
                storage.read_bounded(&handle, 0).await,
                Err(StorageError::SizeLimitExceeded { max_bytes: 0 })
            ));
            storage.write(&handle, b"").await.unwrap();
            assert!(storage.read_bounded(&handle, 0).await.unwrap().is_empty());
            assert!(storage.read(&handle).await.unwrap().is_empty());
        });
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn bounded_file_reads_reject_oversized_contents_and_accept_boundaries() {
        block_on(async {
            let root = tempdir().unwrap();
            let storage = FileStorage::new();
            let handle = StorageHandle::File(root.path().join("bounded.bin"));
            assert!(matches!(
                storage.read_bounded(&handle, 0).await,
                Err(StorageError::NotFound)
            ));
            storage.write(&handle, b"abc").await.unwrap();
            assert_eq!(storage.read_bounded(&handle, 3).await.unwrap(), b"abc");
            assert!(matches!(
                storage.read_bounded(&handle, 2).await,
                Err(StorageError::SizeLimitExceeded { max_bytes: 2 })
            ));
            assert_eq!(storage.read(&handle).await.unwrap(), b"abc");
            storage.write(&handle, b"").await.unwrap();
            assert!(storage.read_bounded(&handle, 0).await.unwrap().is_empty());
        });
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn cache_directory_transaction_serializes_independent_writers_and_releases_lock() {
        let root = tempdir().unwrap();
        let directory = StorageHandle::File(root.path().join("Unicode cache é"));
        let storage = FileStorage::new();
        let transaction = storage.lock_cache_directory(&directory).unwrap();
        let other = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(transaction.directory().join(".cache-lock"))
            .unwrap();
        assert!(matches!(
            other.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        let counter = StorageHandle::File(transaction.directory().join("counter.bin"));
        storage.write_sync(&counter, &0_u64.to_le_bytes()).unwrap();
        drop(transaction);
        other.try_lock().unwrap();
        drop(other);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let directory = &directory;
                let counter = &counter;
                scope.spawn(move || {
                    let storage = FileStorage::new();
                    for _ in 0..16 {
                        let _transaction = storage.lock_cache_directory(directory).unwrap();
                        let bytes = block_on(storage.read_bounded(counter, 8)).unwrap();
                        let value = u64::from_le_bytes(bytes.try_into().unwrap());
                        storage
                            .write_sync(counter, &(value + 1).to_le_bytes())
                            .unwrap();
                    }
                });
            }
        });
        let bytes = storage.read_sync(&counter).unwrap();
        assert_eq!(u64::from_le_bytes(bytes.try_into().unwrap()), 128);
        assert!(root.path().join("Unicode cache é/.cache-lock").exists());
        assert!(matches!(
            storage.lock_cache_directory(&StorageHandle::Memory("no directory".into())),
            Err(StorageError::Unsupported(_))
        ));
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn cache_directory_transaction_streams_regular_metadata_without_following_links() {
        let root = tempdir().unwrap();
        let storage = FileStorage::new();
        let transaction = storage
            .lock_cache_directory(&StorageHandle::File(root.path().to_path_buf()))
            .unwrap();
        let file = StorageHandle::File(root.path().join("payload.bin"));
        storage.write_sync(&file, b"abc").unwrap();
        storage
            .ensure_directory_sync(&StorageHandle::File(root.path().join("directory.bin")))
            .unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            root.path().join("payload.bin"),
            root.path().join("link.bin"),
        )
        .unwrap();
        let mut files = transaction
            .files()
            .unwrap()
            .collect::<StorageResult<Vec<_>>>()
            .unwrap();
        files.retain(|entry| {
            entry.handle != StorageHandle::File(transaction.directory().join(".cache-lock"))
        });
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].handle,
            StorageHandle::File(transaction.directory().join("payload.bin"))
        );
        assert_eq!(files[0].bytes, 3);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn cache_directory_transaction_serializes_child_process() {
        const CHILD_ROOT: &str = "LUNCO_STORAGE_CACHE_TRANSACTION_TEST_ROOT";
        if let Some(root) = std::env::var_os(CHILD_ROOT) {
            let root = std::path::PathBuf::from(root);
            let storage = FileStorage::new();
            storage
                .write_sync(&StorageHandle::File(root.join("ready")), b"ready")
                .unwrap();
            let _transaction = storage
                .lock_cache_directory(&StorageHandle::File(root.clone()))
                .unwrap();
            storage
                .write_sync(&StorageHandle::File(root.join("published")), b"complete")
                .unwrap();
            return;
        }
        let root = tempdir().unwrap();
        let storage = FileStorage::new();
        let transaction = storage
            .lock_cache_directory(&StorageHandle::File(root.path().to_path_buf()))
            .unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "file_storage::tests::cache_directory_transaction_serializes_child_process",
            ])
            .env(CHILD_ROOT, root.path())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !root.path().join("ready").exists() {
            if child.try_wait().unwrap().is_some() || std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("child failed to reach the cache transaction");
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!root.path().join("published").exists());
        assert!(child.try_wait().unwrap().is_none());
        drop(transaction);
        assert!(child.wait().unwrap().success());
        assert_eq!(
            storage
                .read_sync(&StorageHandle::File(root.path().join("published")))
                .unwrap(),
            b"complete"
        );
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn bounded_file_reads_cap_growth_after_open() {
        use std::io::{Read, Write};
        use std::sync::{
            Barrier,
            atomic::{AtomicUsize, Ordering},
        };

        struct GrowingFile<'a> {
            file: std::fs::File,
            barrier: &'a Barrier,
            consumed: &'a AtomicUsize,
            growing: bool,
        }
        impl Read for GrowingFile<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                let read = self.file.read(buffer)?;
                self.consumed.fetch_add(read, Ordering::Relaxed);
                if !self.growing {
                    self.growing = true;
                    self.barrier.wait();
                    self.barrier.wait();
                }
                Ok(read)
            }
        }

        let root = tempdir().unwrap();
        let path = root.path().join("growing.bin");
        FileStorage::new()
            .write_sync(&StorageHandle::File(path.clone()), b"a")
            .unwrap();
        let file = std::fs::File::open(&path).unwrap();
        assert_eq!(file.metadata().unwrap().len(), 1);
        let barrier = Barrier::new(2);
        let consumed = AtomicUsize::new(0);
        let result = std::thread::scope(|scope| {
            let writer = scope.spawn(|| {
                barrier.wait();
                let mut file = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .unwrap();
                file.write_all(&[b'b'; 63]).unwrap();
                file.flush().unwrap();
                barrier.wait();
            });
            let result = read_contents(
                GrowingFile {
                    file,
                    barrier: &barrier,
                    consumed: &consumed,
                    growing: false,
                },
                Some(3),
            );
            writer.join().unwrap();
            result
        });
        assert!(matches!(
            result,
            Err(StorageError::SizeLimitExceeded { max_bytes: 3 })
        ));
        assert_eq!(consumed.load(Ordering::Relaxed), 4);
        assert_eq!(std::fs::metadata(path).unwrap().len(), 64);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn missing_file_returns_not_found() {
        block_on(async {
            let s = FileStorage::new();
            let h = StorageHandle::File("/tmp/lunco-storage-does-not-exist.xxx".into());
            assert!(matches!(s.read(&h).await, Err(StorageError::NotFound)));
        });
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn file_roundtrip_through_tempdir() {
        block_on(async {
            let dir = tempdir().unwrap();
            let dir = dir.path();
            let path = dir.join("file.txt");
            let s = FileStorage::new();
            let h = StorageHandle::File(path.clone());
            s.write(&h, b"persisted").await.unwrap();
            assert!(s.exists(&h).await);
            assert_eq!(s.read(&h).await.unwrap(), b"persisted");
        });
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn concurrent_file_replacements_publish_complete_bytes() {
        use std::sync::{
            Barrier,
            atomic::{AtomicBool, Ordering},
        };
        let root = tempdir().unwrap();
        let handle = StorageHandle::File(root.path().join("shared.bin"));
        let storage = FileStorage::new();
        const SIZE: usize = 128 * 1024;
        storage.write_sync(&handle, &vec![0; SIZE]).unwrap();
        let barrier = Barrier::new(4);
        let done = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                while !done.load(Ordering::Acquire) {
                    let bytes = storage.read_sync(&handle).unwrap();
                    assert_eq!(bytes.len(), SIZE);
                    assert!(bytes.iter().all(|byte| *byte == bytes[0]));
                }
            });
            let writers: Vec<_> = (1..=4)
                .map(|value| {
                    let storage = &storage;
                    let handle = &handle;
                    let barrier = &barrier;
                    scope.spawn(move || {
                        let bytes = vec![value; SIZE];
                        barrier.wait();
                        for _ in 0..16 {
                            storage.write_sync(handle, &bytes).unwrap();
                        }
                    })
                })
                .collect();
            let results: Vec<_> = writers.into_iter().map(|writer| writer.join()).collect();
            done.store(true, Ordering::Release);
            reader.join().unwrap();
            for result in results {
                result.unwrap();
            }
        });
        let entries = storage
            .read_directory_sync(&StorageHandle::File(root.path().to_path_buf()))
            .unwrap();
        assert_eq!(entries, vec![handle]);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn file_write_new_commits_without_replacing_an_existing_entry() {
        block_on(async {
            let root = tempdir().unwrap();
            let handle = StorageHandle::File(root.path().join("capture.lcsin"));
            let storage = FileStorage::new();

            storage.write_new(&handle, b"first archive").await.unwrap();
            assert!(matches!(
                storage.write_new(&handle, b"replacement").await,
                Err(StorageError::AlreadyExists)
            ));
            assert_eq!(storage.read(&handle).await.unwrap(), b"first archive");
        });
    }

    #[test]
    fn memory_write_new_commits_without_replacing_an_existing_entry() {
        block_on(async {
            let handle = StorageHandle::Memory("capture".to_owned());
            let storage = FileStorage::new();

            storage.write_new(&handle, b"first archive").await.unwrap();
            assert!(matches!(
                storage.write_new(&handle, b"replacement").await,
                Err(StorageError::AlreadyExists)
            ));
            assert_eq!(storage.read(&handle).await.unwrap(), b"first archive");
        });
    }

    #[test]
    fn memory_unsupported_for_file_only_ops_is_silent() {
        block_on(async {
            // Memory handle should work fine; unsupported variants will be
            // behind feature flags so the test compiles on the default set.
            let s = FileStorage::new();
            let h = StorageHandle::Memory("x".into());
            assert!(s.is_writable(&h).await);
        });
    }

    #[test]
    fn memory_rename_moves_the_entry() {
        block_on(async {
            let s = FileStorage::new();
            let from = StorageHandle::Memory("from".into());
            let to = StorageHandle::Memory("to".into());
            s.write(&from, b"payload").await.unwrap();
            s.rename(&from, &to).await.unwrap();
            assert!(!s.exists(&from).await);
            assert_eq!(s.read(&to).await.unwrap(), b"payload");
        });
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn file_rename_moves_the_entry() {
        block_on(async {
            let dir = tempdir().unwrap();
            let dir = dir.path();
            let from_path = dir.join("from.txt");
            let to_path = dir.join("to.txt");
            let s = FileStorage::new();
            let from = StorageHandle::File(from_path.clone());
            let to = StorageHandle::File(to_path.clone());
            s.write(&from, b"payload").await.unwrap();
            s.rename(&from, &to).await.unwrap();
            assert!(!s.exists(&from).await);
            assert_eq!(s.read(&to).await.unwrap(), b"payload");
        });
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn entry_kind_and_directory_creation_stay_in_the_backend() {
        block_on(async {
            let root = tempdir().unwrap();
            let root_path = root.path();
            let nested = root_path.join("nested");
            let file_path = nested.join("scene.usda");
            let s = FileStorage::new();
            let directory = StorageHandle::File(nested.clone());
            let file = StorageHandle::File(file_path.clone());

            s.ensure_directory(&directory).await.unwrap();
            assert_eq!(
                s.entry_kind(&directory).await.unwrap(),
                StorageEntryKind::Directory
            );
            assert_eq!(
                s.read_directory(&StorageHandle::File(root_path.to_path_buf()))
                    .await
                    .unwrap(),
                vec![StorageHandle::File(nested.clone())]
            );
            s.write(&file, b"#usda 1.0\n").await.unwrap();
            assert_eq!(s.entry_kind(&file).await.unwrap(), StorageEntryKind::File);
        });
    }
}
