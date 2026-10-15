//! Native asset containment around Bevy's maintained streaming file reader.

use bevy::asset::io::{AssetReader, AssetReaderError, PathStream, Reader, file::FileAssetReader};
use std::path::{Path, PathBuf};

/// Shared native source adapter. Missing roots are allowed for unpopulated
/// cache tiers; existing paths must resolve inside their owning directory.
pub struct NativeAssetReader {
    root: PathBuf,
    reader: FileAssetReader,
}

impl NativeAssetReader {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            reader: FileAssetReader::new(&root),
            root,
        }
    }

    fn resolve(&self, path: &Path) -> Result<PathBuf, AssetReaderError> {
        if !self.root.is_absolute() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "native asset source root must be absolute",
            )
            .into());
        }
        crate::existing_path_within_root(&self.root, path)?
            .ok_or_else(|| AssetReaderError::NotFound(path.to_path_buf()))
    }

    fn regular_file(&self, path: &Path) -> Result<(), AssetReaderError> {
        let resolved = self.resolve(path)?;
        if !std::fs::metadata(resolved)?.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "asset reads require a regular file",
            )
            .into());
        }
        Ok(())
    }
}

impl AssetReader for NativeAssetReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        self.regular_file(path)?;
        self.reader.read(path).await
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        // Bevy's sidecar-path helper is private. Validate its `.meta` sibling
        // before delegating so sidecars obey the same containment as payloads.
        let mut meta = path.to_path_buf();
        let mut extension = path.extension().unwrap_or_default().to_os_string();
        if !extension.is_empty() {
            extension.push(".");
        }
        extension.push("meta");
        meta.set_extension(extension);
        self.regular_file(&meta)?;
        self.reader.read_meta(path).await
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        self.resolve(path)?;
        self.reader.read_directory(path).await
    }

    async fn is_directory<'a>(&'a self, path: &'a Path) -> Result<bool, AssetReaderError> {
        match self.resolve(path) {
            Ok(path) => Ok(std::fs::metadata(path)?.is_dir()),
            Err(AssetReaderError::NotFound(_)) => Ok(false),
            Err(error) => Err(error),
        }
    }
}
