//! Bounded byte access through the authoritative registered asset sources.

use bevy::asset::{AssetPath, AssetServer, ReadAssetBytesError};

/// Read once through the authoritative source selected by a typed path.
pub async fn read_registered_asset_bytes(
    asset_server: &AssetServer,
    path: &AssetPath<'_>,
    max_bytes: usize,
) -> Result<Vec<u8>, ReadAssetBytesError> {
    let source = asset_server.get_source(path.source().clone())?;
    let reader = source.reader().read(path.path()).await?;
    use bevy::tasks::futures_lite::io::AsyncReadExt;
    let limit = u64::try_from(max_bytes)
        .ok()
        .and_then(|limit| limit.checked_add(1))
        .ok_or_else(|| ReadAssetBytesError::Io {
            path: path.path().to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "asset byte limit overflows the reader bound",
            ),
        })?;
    let mut reader = reader.take(limit);
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let count = reader
            .read(&mut buffer)
            .await
            .map_err(|source| ReadAssetBytesError::Io {
                path: path.path().to_path_buf(),
                source,
            })?;
        if count == 0 {
            break;
        }
        if count > max_bytes.saturating_sub(bytes.len()) {
            return Err(ReadAssetBytesError::Io {
                path: path.path().to_path_buf(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("asset exceeds the {max_bytes}-byte preparation budget"),
                ),
            });
        }
        bytes
            .try_reserve_exact(count)
            .map_err(|error| ReadAssetBytesError::Io {
                path: path.path().to_path_buf(),
                source: std::io::Error::other(format!("asset buffer allocation failed: {error}")),
            })?;
        bytes.extend_from_slice(&buffer[..count]);
    }
    Ok(bytes)
}

/// A prepared storage location from the shared asset owner. Language adapters
/// select an address; this boundary owns every byte read and its budget.
#[derive(Clone, Debug)]
pub enum AssetReadTarget {
    Registered(AssetPath<'static>),
    /// Explicit native file identity admitted by the storage URI boundary.
    /// Native filenames do not pass through portable asset-component rules.
    File(std::path::PathBuf),
    Directory {
        directory: crate::AssetDirectory,
        relative: std::path::PathBuf,
    },
}

impl AssetReadTarget {
    pub fn path(&self) -> &std::path::Path {
        match self {
            Self::Registered(path) => path.path(),
            Self::File(path) => path.as_path(),
            Self::Directory { relative, .. } => relative,
        }
    }

    pub async fn read(&self, server: &AssetServer, max_bytes: usize) -> Result<Vec<u8>, String> {
        match self {
            Self::File(path) => {
                if !path.is_absolute() {
                    return Err("native file identity must be absolute".into());
                }
                #[cfg(not(target_arch = "wasm32"))]
                return lunco_storage::FileStorage::new()
                    .read_bounded(&lunco_storage::StorageHandle::File(path.clone()), max_bytes)
                    .await
                    .map_err(|error| format!("{}: {error}", path.display()));
                #[cfg(target_arch = "wasm32")]
                return Err("native file identities require a native host".into());
            }
            Self::Registered(path) => read_registered_asset_bytes(server, path, max_bytes)
                .await
                .map_err(|error| format!("{path}: {error}")),
            Self::Directory {
                directory,
                relative,
            } => {
                let path = directory
                    .file_path_on_worker(relative)
                    .map_err(|error| error.to_string())?;
                #[cfg(not(target_arch = "wasm32"))]
                return lunco_storage::FileStorage::new()
                    .read_bounded(&lunco_storage::StorageHandle::File(path), max_bytes)
                    .await
                    .map_err(|error| error.to_string());
                #[cfg(target_arch = "wasm32")]
                match directory {
                    crate::AssetDirectory::File(_) => lunco_storage::OpfsStorage::new()
                        .read_bounded(&lunco_storage::StorageHandle::File(path), max_bytes)
                        .await
                        .map_err(|error| error.to_string()),
                    crate::AssetDirectory::Web(base) => {
                        let path = base.join(relative);
                        let path = AssetPath::from_path_buf(path).with_source(crate::LUNCO_SCHEME);
                        read_registered_asset_bytes(server, &path, max_bytes)
                            .await
                            .map_err(|error| error.to_string())
                    }
                }
            }
        }
    }
}
