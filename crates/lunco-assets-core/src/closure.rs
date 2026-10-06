//! Bounded transitive file traversal through native storage. Format owners
//! classify documents and extract dependencies; this owner admits paths and
//! limits source bytes before parsing them.

#[cfg(not(target_arch = "wasm32"))]
use std::collections::BTreeSet;
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};

#[cfg(not(target_arch = "wasm32"))]
use crate::asset_path;

/// Budgets for one native dependency walk: admitted file count and source bytes,
/// not parser-internal allocations or leaf payloads.
#[derive(bevy::prelude::Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileClosureLimits {
    pub max_files: usize,
    pub max_document_bytes: usize,
    pub max_total_document_bytes: usize,
}
impl Default for FileClosureLimits {
    fn default() -> Self {
        Self {
            max_files: 4096,
            max_document_bytes: 16 * 1024 * 1024,
            max_total_document_bytes: 64 * 1024 * 1024,
        }
    }
}
#[cfg(not(target_arch = "wasm32"))]
impl FileClosureLimits {
    pub fn validate(&self) -> Result<(), FileClosureError> {
        if self.max_files == 0 || self.max_document_bytes == 0 || self.max_total_document_bytes == 0
        {
            Err(FileClosureError::InvalidLimits)
        } else {
            Ok(())
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, thiserror::Error)]
pub enum FileClosureError {
    #[error("asset closure limits must be positive")]
    InvalidLimits,
    #[error("asset closure exceeds {limit} admitted files")]
    FileLimit { limit: usize },
    #[error("cannot read asset document `{path}`: {error}")]
    Read {
        path: PathBuf,
        #[source]
        error: lunco_storage::StorageError,
    },
    #[error("asset document `{path}` is not UTF-8: {error}")]
    Utf8 {
        path: PathBuf,
        #[source]
        error: std::string::FromUtf8Error,
    },
    #[error("asset document `{path}` could not be parsed")]
    Parse { path: PathBuf },
}

/// Walk every file transitively named by `roots`.
///
/// `is_document` decides whether a file may contain further dependencies;
/// `dependencies` receives its UTF-8 source and returns those raw references.
/// Anchored references are intentionally skipped because this form has no
/// mapping for a scheme or an assets-root-relative path.
#[cfg(not(target_arch = "wasm32"))]
pub fn transitive_file_closure(
    roots: &[PathBuf],
    limits: &FileClosureLimits,
    is_document: impl Fn(&Path) -> bool,
    dependencies: impl Fn(&str) -> Option<Vec<String>>,
) -> Result<BTreeSet<PathBuf>, FileClosureError> {
    transitive_file_closure_with(roots, limits, |_| None, is_document, dependencies)
}

/// [`transitive_file_closure`] with resolution for anchored references.
///
/// The resolver is given the unmodified reference and may return a local file
/// for it. Returning `None` leaves that dependency out of the local closure.
#[cfg(not(target_arch = "wasm32"))]
pub fn transitive_file_closure_with(
    roots: &[PathBuf],
    limits: &FileClosureLimits,
    resolve_anchored: impl Fn(&str) -> Option<PathBuf>,
    is_document: impl Fn(&Path) -> bool,
    dependencies: impl Fn(&str) -> Option<Vec<String>>,
) -> Result<BTreeSet<PathBuf>, FileClosureError> {
    limits.validate()?;
    let mut admitted = BTreeSet::new();
    let mut queue = Vec::new();
    let admit = |path: PathBuf, admitted: &mut BTreeSet<PathBuf>, queue: &mut Vec<PathBuf>| {
        let path = asset_path::normalize(&path);
        if !admitted.contains(&path) {
            if admitted.len() >= limits.max_files {
                return Err(FileClosureError::FileLimit {
                    limit: limits.max_files,
                });
            }
            admitted.insert(path.clone());
            queue.push(path);
        }
        Ok(())
    };
    for root in roots {
        admit(root.clone(), &mut admitted, &mut queue)?;
    }
    let storage = lunco_storage::FileStorage::new();
    let mut remaining = limits.max_total_document_bytes;

    while let Some(path) = queue.pop() {
        if !is_document(&path) {
            continue;
        }
        let bytes = bevy::tasks::futures_lite::future::block_on(storage.read_bounded(
            &lunco_storage::StorageHandle::File(path.clone()),
            remaining.min(limits.max_document_bytes),
        ))
        .map_err(|error| FileClosureError::Read {
            path: path.clone(),
            error,
        })?;
        remaining -= bytes.len();
        let text = String::from_utf8(bytes).map_err(|error| FileClosureError::Utf8 {
            path: path.clone(),
            error,
        })?;
        let arcs =
            dependencies(&text).ok_or_else(|| FileClosureError::Parse { path: path.clone() })?;
        let base = path.parent().unwrap_or_else(|| Path::new(""));
        for arc in arcs {
            if asset_path::is_anchored(&arc) {
                if let Some(resolved) = resolve_anchored(&arc) {
                    admit(resolved, &mut admitted, &mut queue)?;
                }
            } else {
                admit(base.join(arc), &mut admitted, &mut queue)?;
            }
        }
    }
    Ok(admitted)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use lunco_storage::Storage;
    #[test]
    fn file_closure_budgets_and_document_errors_are_terminal() {
        let root = tempfile::tempdir().unwrap();
        let document = root.path().join("source.node");
        let storage = lunco_storage::FileStorage::new();
        let write = |bytes: &[u8]| {
            storage
                .write_sync(&lunco_storage::StorageHandle::File(document.clone()), bytes)
                .unwrap()
        };
        let is_document = |path: &Path| {
            path.extension()
                .is_some_and(|extension| extension == "node")
        };
        let dependencies = |text: &str| Some(vec![text.to_owned()]);
        let limits = FileClosureLimits {
            max_files: 2,
            max_document_bytes: 16,
            max_total_document_bytes: 16,
        };
        write(b"missing.bin");
        assert!(
            transitive_file_closure(&[document.clone()], &limits, is_document, dependencies)
                .unwrap()
                .contains(&root.path().join("missing.bin"))
        );
        assert!(matches!(
            transitive_file_closure(
                &[document.clone()],
                &FileClosureLimits {
                    max_files: 1,
                    ..limits
                },
                is_document,
                dependencies
            ),
            Err(FileClosureError::FileLimit { .. })
        ));
        assert!(matches!(
            transitive_file_closure(
                &[document.clone()],
                &FileClosureLimits {
                    max_document_bytes: 2,
                    ..limits
                },
                is_document,
                dependencies
            ),
            Err(FileClosureError::Read {
                error: lunco_storage::StorageError::SizeLimitExceeded { .. },
                ..
            })
        ));
        assert!(matches!(
            transitive_file_closure(&[document.clone()], &limits, is_document, |_| None),
            Err(FileClosureError::Parse { .. })
        ));
        write(&[0xff]);
        assert!(matches!(
            transitive_file_closure(&[document.clone()], &limits, is_document, dependencies),
            Err(FileClosureError::Utf8 { .. })
        ));
        assert!(matches!(
            transitive_file_closure(
                &[root.path().join("absent.node")],
                &limits,
                is_document,
                dependencies
            ),
            Err(FileClosureError::Read {
                error: lunco_storage::StorageError::NotFound,
                ..
            })
        ));
        assert!(matches!(
            transitive_file_closure(
                &[],
                &FileClosureLimits {
                    max_files: 0,
                    ..limits
                },
                is_document,
                dependencies
            ),
            Err(FileClosureError::InvalidLimits)
        ));
        write(b"child.node");
        storage
            .write_sync(
                &lunco_storage::StorageHandle::File(root.path().join("child.node")),
                b"xx",
            )
            .unwrap();
        assert!(matches!(
            transitive_file_closure(
                &[document],
                &FileClosureLimits {
                    max_total_document_bytes: 11,
                    ..limits
                },
                is_document,
                dependencies
            ),
            Err(FileClosureError::Read {
                error: lunco_storage::StorageError::SizeLimitExceeded { .. },
                ..
            })
        ));
    }
}
