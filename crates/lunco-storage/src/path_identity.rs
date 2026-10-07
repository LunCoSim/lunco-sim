//! Worker-prepared filesystem identities for output ownership checks.
//!
//! Canonical existing ancestors preserve native aliases. Missing descendants
//! inherit their ancestor's case semantics; comparisons perform no I/O.

use std::ffi::OsString;
use std::path::{Component, Path};

use crate::{StorageError, StorageResult};

#[derive(Clone, Debug)]
enum CaseSensitivity {
    Sensitive,
    #[cfg(windows)]
    Insensitive,
    #[cfg(windows)]
    Unknown(String),
}

/// Native destination identity, including paths not created yet.
///
/// Prepare on the owning I/O worker and carry the result through admission.
/// This is an ownership snapshot, not a lock against external filesystem edits.
#[derive(Clone, Debug)]
pub struct FilePathIdentity {
    components: Vec<OsString>,
    existing: usize,
    sensitivity: CaseSensitivity,
}

impl FilePathIdentity {
    /// Resolve aliases through the deepest existing ancestor without creating it.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn prepare(path: &Path) -> StorageResult<Self> {
        let absolute = std::path::absolute(path)?;
        let mut ancestor = absolute.as_path();
        let mut missing = Vec::new();
        let canonical = loop {
            match std::fs::canonicalize(ancestor) {
                Ok(path) => break path,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    match std::fs::symlink_metadata(ancestor) {
                        Ok(_) => return Err(StorageError::Io(error)),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Err(StorageError::Io(error)),
                    }
                    let name = ancestor.file_name().ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "destination has no existing filesystem ancestor",
                        )
                    })?;
                    missing.push(name.to_os_string());
                    ancestor = ancestor.parent().ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "destination has no parent",
                        )
                    })?;
                }
                Err(error) => return Err(StorageError::Io(error)),
            }
        };
        let sensitivity = if missing.is_empty() {
            CaseSensitivity::Sensitive
        } else {
            if !canonical.is_dir() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotADirectory,
                    "destination ancestor is not a directory",
                )
                .into());
            }
            directory_sensitivity(&canonical)
        };
        let mut components: Vec<_> = canonical
            .components()
            .map(|c| c.as_os_str().to_os_string())
            .collect();
        let existing = components.len();
        components.extend(missing.into_iter().rev());
        Ok(Self {
            components,
            existing,
            sensitivity,
        })
    }

    /// OPFS has exact logical component identity and no native aliases.
    #[cfg(target_arch = "wasm32")]
    pub fn prepare(path: &Path) -> StorageResult<Self> {
        let canonical = crate::canonicalize_file_path(path)?;
        let components: Vec<_> = canonical
            .components()
            .map(|c| c.as_os_str().to_os_string())
            .collect();
        Ok(Self {
            existing: components.len(),
            components,
            sensitivity: CaseSensitivity::Sensitive,
        })
    }

    /// Whether one identity owns the other identity or an ancestor of it.
    pub fn overlaps(&self, other: &Self) -> StorageResult<bool> {
        for (index, (left, right)) in self.components.iter().zip(&other.components).enumerate() {
            if left == right {
                continue;
            }
            // Native Windows volume/share identities are case-insensitive.
            let prefix = matches!(
                Path::new(left).components().next(),
                Some(Component::Prefix(_))
            );
            if prefix {
                if !component_equal(left, right, false)? {
                    return Ok(false);
                }
                continue;
            }
            let sensitivity = if index >= self.existing {
                &self.sensitivity
            } else if index >= other.existing {
                &other.sensitivity
            } else {
                // Existing canonical names came from the filesystem itself.
                return Ok(false);
            };
            match sensitivity {
                CaseSensitivity::Sensitive => return Ok(false),
                #[cfg(windows)]
                CaseSensitivity::Insensitive => {
                    if !component_equal(left, right, false)? {
                        return Ok(false);
                    }
                }
                #[cfg(windows)]
                CaseSensitivity::Unknown(detail) => {
                    if component_equal(left, right, false)? {
                        return Err(StorageError::Unsupported(format!(
                            "cannot establish destination case identity: {detail}"
                        )));
                    }
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}

#[cfg(not(windows))]
fn directory_sensitivity(_path: &Path) -> CaseSensitivity {
    CaseSensitivity::Sensitive
}

#[cfg(windows)]
fn directory_sensitivity(path: &Path) -> CaseSensitivity {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_CASE_SENSITIVE_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES,
        FileCaseSensitiveInfo, GetFileInformationByHandleEx,
    };
    use windows_sys::Win32::System::SystemServices::FILE_CS_FLAG_CASE_SENSITIVE_DIR;
    let result = (|| -> std::io::Result<bool> {
        let file = std::fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)?;
        let mut info = FILE_CASE_SENSITIVE_INFO::default();
        // SAFETY: the live handle and correctly sized structure outlive the call.
        let success = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileCaseSensitiveInfo,
                (&mut info as *mut FILE_CASE_SENSITIVE_INFO).cast(),
                std::mem::size_of_val(&info) as u32,
            )
        };
        if success == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(info.Flags & FILE_CS_FLAG_CASE_SENSITIVE_DIR != 0)
    })();
    match result {
        Ok(true) => CaseSensitivity::Sensitive,
        Ok(false) => CaseSensitivity::Insensitive,
        Err(error) => CaseSensitivity::Unknown(format!("{}: {error}", path.display())),
    }
}

#[cfg(windows)]
fn component_equal(
    left: &std::ffi::OsStr,
    right: &std::ffi::OsStr,
    sensitive: bool,
) -> StorageResult<bool> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
    let left: Vec<_> = left.encode_wide().collect();
    let right: Vec<_> = right.encode_wide().collect();
    let length = |n| {
        i32::try_from(n).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "filesystem component exceeds Windows comparison range",
            )
        })
    };
    // SAFETY: both slices are live UTF-16 buffers with the supplied lengths.
    let result = unsafe {
        CompareStringOrdinal(
            left.as_ptr(),
            length(left.len())?,
            right.as_ptr(),
            length(right.len())?,
            i32::from(!sensitive),
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(result == CSTR_EQUAL)
}

#[cfg(not(windows))]
fn component_equal(
    left: &std::ffi::OsStr,
    right: &std::ffi::OsStr,
    _sensitive: bool,
) -> StorageResult<bool> {
    Ok(left == right)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn destination_identity_covers_missing_descendants_and_native_aliases() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("Site");
        std::fs::create_dir(&parent).unwrap();
        let identity = FilePathIdentity::prepare(&parent).unwrap();
        assert!(
            identity
                .overlaps(&FilePathIdentity::prepare(&parent.join("missing/texture.png")).unwrap())
                .unwrap()
        );
        assert!(
            !identity
                .overlaps(
                    &FilePathIdentity::prepare(&root.path().join("Other/texture.png")).unwrap()
                )
                .unwrap()
        );
        let alias =
            FilePathIdentity::prepare(&root.path().join("site/missing/texture.png")).unwrap();
        assert_eq!(identity.overlaps(&alias).unwrap(), cfg!(windows));
        let missing =
            FilePathIdentity::prepare(&root.path().join("uncreated/texture.png")).unwrap();
        let other = FilePathIdentity::prepare(&root.path().join("UNCREATED/Texture.png")).unwrap();
        assert_eq!(missing.overlaps(&other).unwrap(), cfg!(windows));
        #[cfg(unix)]
        {
            let link = root.path().join("linked");
            std::os::unix::fs::symlink(&parent, &link).unwrap();
            assert!(
                identity
                    .overlaps(&FilePathIdentity::prepare(&link.join("missing/a")).unwrap())
                    .unwrap()
            );
            std::os::unix::fs::symlink(root.path().join("absent"), root.path().join("broken"))
                .unwrap();
            assert!(FilePathIdentity::prepare(&root.path().join("broken/a")).is_err());
        }
    }
}
