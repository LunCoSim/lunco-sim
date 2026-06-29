//! Native filesystem backend for [`crate::Storage`].
//!
//! Reads / writes via `std::fs`. Only handles [`StorageHandle::File`] and
//! [`StorageHandle::Memory`] variants — other variants return
//! [`StorageError::Unsupported`]. (File-open/save pickers are a UI concern and
//! live in `lunco_workbench::picker`, not on the `Storage` trait.)
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
use std::sync::Mutex;

use crate::{Storage, StorageError, StorageHandle, StorageResult};

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
    /// Construct a fresh backend.
    pub fn new() -> Self {
        Self::default()
    }
}

/// Atomic file replace (tmp + `fsync` + `rename`) — the implementation
/// behind the `File` arm of [`FileStorage::write`] (CQ-107). Private: the
/// world reaches this through the [`Storage`] API (`write` / `write_sync`),
/// never as a bare-path bypass, so the backend abstraction holds. A crash
/// mid-write leaves the prior file intact, never a truncated one. The temp
/// is a hidden per-process sibling so the rename stays within one
/// filesystem and won't collide with a concurrent writer's temp.
#[cfg(not(target_arch = "wasm32"))]
fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("tmp");
    let tmp = path.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

#[async_trait::async_trait]
impl Storage for FileStorage {
    async fn read(&self, handle: &StorageHandle) -> StorageResult<Vec<u8>> {
        match handle {
            #[cfg(not(target_arch = "wasm32"))]
            StorageHandle::File(path) => match std::fs::read(path) {
                Ok(bytes) => Ok(bytes),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    Err(StorageError::NotFound)
                }
                Err(e) => Err(StorageError::Io(e)),
            },
            StorageHandle::Memory(key) => {
                let map = self.memory.lock().expect("memory poisoned");
                map.get(key)
                    .cloned()
                    .ok_or(StorageError::NotFound)
            }
            _ => Err(StorageError::Unsupported(
                "FileStorage does not handle web / remote variants".into(),
            )),
        }
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
                let mut map = self.memory.lock().expect("memory poisoned");
                map.insert(key.clone(), bytes.to_vec());
                Ok(())
            }
            _ => Err(StorageError::Unsupported(
                "FileStorage does not handle web / remote variants".into(),
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
                .map(|m| m.contains_key(key))
                .unwrap_or(false),
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
            let dir = std::env::temp_dir().join("lunco-storage-test-rt");
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("file.txt");
            let s = FileStorage::new();
            let h = StorageHandle::File(path.clone());
            s.write(&h, b"persisted").await.unwrap();
            assert!(s.exists(&h).await);
            assert_eq!(s.read(&h).await.unwrap(), b"persisted");
            let _ = std::fs::remove_file(&path);
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
}
