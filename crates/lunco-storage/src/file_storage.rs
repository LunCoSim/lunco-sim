//! Native filesystem backend for [`crate::Storage`].
//!
//! Reads / writes via `std::fs`; pickers via `rfd::FileDialog`. Only
//! handles [`StorageHandle::File`] and [`StorageHandle::Memory`]
//! variants — other variants return [`StorageError::Unsupported`].
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

use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
use std::sync::Mutex;

use crate::{OpenFilter, SaveHint, Storage, StorageError, StorageHandle, StorageResult};

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

impl Storage for FileStorage {
    fn read(&self, handle: &StorageHandle) -> StorageResult<Vec<u8>> {
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

    fn write(&self, handle: &StorageHandle, bytes: &[u8]) -> StorageResult<()> {
        match handle {
            #[cfg(not(target_arch = "wasm32"))]
            StorageHandle::File(path) => {
                // Ensure parent dir exists — Save-As into a fresh folder
                // shouldn't require the user to mkdir beforehand.
                if let Some(parent) = path.parent() {
                    if !parent.as_os_str().is_empty() {
                        std::fs::create_dir_all(parent)?;
                    }
                }
                std::fs::write(path, bytes)?;
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

    fn exists(&self, handle: &StorageHandle) -> bool {
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

    fn is_writable(&self, handle: &StorageHandle) -> bool {
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

    fn pick_open(
        &self,
        #[allow(unused_variables)]
        filter: &OpenFilter,
    ) -> StorageResult<Option<StorageHandle>> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut dialog = rfd::FileDialog::new();
            let exts: Vec<&str> =
                filter.extensions.iter().map(|s| s.as_str()).collect();
            if !exts.is_empty() {
                dialog = dialog.add_filter(&filter.name, &exts);
            }
            Ok(dialog.pick_file().map(StorageHandle::File))
        }
        #[cfg(target_arch = "wasm32")]
        {
            Err(StorageError::Unsupported("Native pickers unavailable on wasm32".into()))
        }
    }

    fn pick_save(
        &self,
        #[allow(unused_variables)]
        hint: &SaveHint,
    ) -> StorageResult<Option<StorageHandle>> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut dialog = rfd::FileDialog::new();
            if let Some(name) = &hint.suggested_name {
                dialog = dialog.set_file_name(name);
            }
            if let Some(StorageHandle::File(dir)) = &hint.start_dir {
                let start: PathBuf = if dir.is_dir() {
                    dir.clone()
                } else {
                    dir.parent().map(PathBuf::from).unwrap_or_default()
                };
                if !start.as_os_str().is_empty() {
                    dialog = dialog.set_directory(&start);
                }
            }
            for f in &hint.filters {
                let exts: Vec<&str> = f.extensions.iter().map(|s| s.as_str()).collect();
                if !exts.is_empty() {
                    dialog = dialog.add_filter(&f.name, &exts);
                }
            }
            Ok(dialog.save_file().map(StorageHandle::File))
        }
        #[cfg(target_arch = "wasm32")]
        {
            Err(StorageError::Unsupported("Native pickers unavailable on wasm32".into()))
        }
    }

    fn pick_folder(&self) -> StorageResult<Option<StorageHandle>> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Ok(rfd::FileDialog::new().pick_folder().map(StorageHandle::File))
        }
        #[cfg(target_arch = "wasm32")]
        {
            Err(StorageError::Unsupported("Native pickers unavailable on wasm32".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_roundtrip() {
        let s = FileStorage::new();
        let h = StorageHandle::Memory("k".into());
        assert!(!s.exists(&h));
        s.write(&h, b"hello").unwrap();
        assert!(s.exists(&h));
        assert_eq!(s.read(&h).unwrap(), b"hello");
        s.write(&h, b"world").unwrap();
        assert_eq!(s.read(&h).unwrap(), b"world");
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn missing_file_returns_not_found() {
        let s = FileStorage::new();
        let h = StorageHandle::File("/tmp/lunco-storage-does-not-exist.xxx".into());
        assert!(matches!(s.read(&h), Err(StorageError::NotFound)));
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn file_roundtrip_through_tempdir() {
        let dir = std::env::temp_dir().join("lunco-storage-test-rt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("file.txt");
        let s = FileStorage::new();
        let h = StorageHandle::File(path.clone());
        s.write(&h, b"persisted").unwrap();
        assert!(s.exists(&h));
        assert_eq!(s.read(&h).unwrap(), b"persisted");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn memory_unsupported_for_file_only_ops_is_silent() {
        // Memory handle should work fine; unsupported variants will be
        // behind feature flags so the test compiles on the default set.
        let s = FileStorage::new();
        let h = StorageHandle::Memory("x".into());
        assert!(s.is_writable(&h));
    }
}
