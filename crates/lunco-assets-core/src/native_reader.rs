//! Native asset containment and demand-driven reads on the asset I/O pool.

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
}

/// An owned handle uses the same admission as direct storage reads. Payload
/// reads run on the asset I/O workers and consume only the requested buffer;
/// there is no independent read-ahead allocation outside the consumer budget.
pub(crate) fn open_asset_file(path: &Path) -> Result<impl Reader + use<>, AssetReaderError> {
    lunco_storage::FileStorage::open_regular_file(path)
        .map(NativeFileReader)
        .map_err(|error| match error {
            lunco_storage::StorageError::NotFound => AssetReaderError::NotFound(path.to_path_buf()),
            error => std::io::Error::other(error.to_string()).into(),
        })
}

struct NativeFileReader(std::fs::File);

impl bevy::tasks::futures_lite::io::AsyncRead for NativeFileReader {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buffer: &mut [u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::task::Poll::Ready(std::io::Read::read(&mut self.0, buffer))
    }
}

impl bevy::tasks::futures_lite::io::AsyncSeek for NativeFileReader {
    fn poll_seek(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        position: std::io::SeekFrom,
    ) -> std::task::Poll<std::io::Result<u64>> {
        std::task::Poll::Ready(std::io::Seek::seek(&mut self.0, position))
    }
}

impl Reader for NativeFileReader {
    fn seekable(
        &mut self,
    ) -> Result<&mut dyn bevy::asset::io::SeekableReader, bevy::asset::io::ReaderNotSeekableError>
    {
        Ok(self)
    }
}

impl AssetReader for NativeAssetReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        open_asset_file(&self.resolve(path)?)
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        // Bevy's sidecar-path helper is private. Validate its `.meta` sibling
        // so sidecars obey the same containment and regular-file admission.
        let mut meta = path.to_path_buf();
        let mut extension = path.extension().unwrap_or_default().to_os_string();
        if !extension.is_empty() {
            extension.push(".");
        }
        extension.push("meta");
        meta.set_extension(extension);
        open_asset_file(&self.resolve(&meta)?)
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
