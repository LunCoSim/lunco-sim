//! In-memory and persistent prepared-solve cache for the Modelica worker.
//!
//! Library admission clears worker-local solve models. Persistent entries stay
//! usable because their keys include the deterministic admitted-library
//! revision, solver identity, structural source identity, and parameter values.

#[cfg(not(target_arch = "wasm32"))]
use super::PREPARED_SOLVE_CACHE_VERSION;
use super::solver;
#[cfg(not(target_arch = "wasm32"))]
use lunco_assets_core::modelica_dir;
#[cfg(not(target_arch = "wasm32"))]
use lunco_storage::{FileStorage, Storage, StorageError, StorageHandle};
#[cfg(not(target_arch = "wasm32"))]
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::num::NonZeroUsize;
use std::sync::Arc;

/// Count-bounded FIFO reuse. Only distinct insertions change admission order;
/// replacing or reading a key does not reorder it. Values are owned once and
/// borrowed on lookup, so eviction releases only the cache's reference.
pub(super) struct BoundedReuseCache<K, V> {
    entries: HashMap<K, V>,
    admission_order: VecDeque<K>,
    capacity: NonZeroUsize,
}

impl<K: Clone + Eq + Hash, V> BoundedReuseCache<K, V> {
    pub(super) fn new(capacity: NonZeroUsize) -> Self {
        Self {
            entries: HashMap::new(),
            admission_order: VecDeque::new(),
            capacity,
        }
    }

    pub(super) fn contains_key(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    pub(super) fn get(&self, key: &K) -> Option<&V> {
        self.entries.get(key)
    }

    pub(super) fn insert(&mut self, key: K, value: V) {
        if !self.entries.contains_key(&key) {
            if self.entries.len() == self.capacity.get() {
                // Each distinct insertion has exactly one queue entry.
                let oldest = self
                    .admission_order
                    .pop_front()
                    .expect("nonempty bounded cache has an admitted oldest key");
                self.entries.remove(&oldest);
            }
            self.admission_order.push_back(key.clone());
        }
        self.entries.insert(key, value);
    }

    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.admission_order.clear();
    }
}

pub(super) type CompiledArtifactCache =
    BoundedReuseCache<u64, Box<rumoca_compile::compile::DaeCompilationResult>>;

/// Native optional solve-cache byte and retention budgets captured at worker startup.
/// Insert this resource before `ModelicaExecutionPlugin` to change the budgets.
/// The codec ceiling is an admission invariant; a larger artifact is recomputed
/// from its admitted DAE instead of being loaded from optional storage.
#[cfg(not(target_arch = "wasm32"))]
#[derive(bevy::prelude::Resource, Clone, Copy, Debug)]
pub struct PreparedSolveDiskLimits {
    /// Maximum regular v5 cache records retained after a successful publication.
    pub retained_entries: usize,
    pub compressed_bytes: usize,
    pub decoded_bytes: usize,
    /// Maximum zstd back-reference window, expressed as a base-two logarithm.
    pub zstd_window_log_max: u32,
}

#[cfg(not(target_arch = "wasm32"))]
impl PreparedSolveDiskLimits {
    pub const CODEC_MAX_DECODED_BYTES: usize = 256 * 1024 * 1024;
    pub const MAX_WINDOW_LOG: u32 = 28;

    pub fn validate(&self) -> Result<(), String> {
        if self.retained_entries == 0
            || self.compressed_bytes == 0
            || self.compressed_bytes > Self::CODEC_MAX_DECODED_BYTES
            || self.decoded_bytes == 0
            || self.decoded_bytes > Self::CODEC_MAX_DECODED_BYTES
            || !(10..=Self::MAX_WINDOW_LOG).contains(&self.zstd_window_log_max)
        {
            return Err(format!(
                "invalid prepared-solve disk-cache limits: retained entries must be positive, compressed/decoded byte budgets must be 1..={} and zstd window log must be 10..={}",
                Self::CODEC_MAX_DECODED_BYTES,
                Self::MAX_WINDOW_LOG,
            ));
        }
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for PreparedSolveDiskLimits {
    fn default() -> Self {
        Self {
            retained_entries: 32,
            compressed_bytes: 64 * 1024 * 1024,
            decoded_bytes: Self::CODEC_MAX_DECODED_BYTES,
            zstd_window_log_max: 26,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub(super) enum PreparedSolveCacheReadError {
    InvalidLimits(String),
    Storage(StorageError),
    Decompression(std::io::Error),
    DecodedLimit { max_bytes: usize },
    Codec(bincode::error::DecodeError),
    TrailingBytes,
    Identity,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub(super) enum PreparedSolveCacheWriteError {
    InvalidLimits(String),
    InvalidPath,
    Codec(bincode::error::EncodeError),
    Compression(std::io::Error),
    Storage(StorageError),
}

#[cfg(not(target_arch = "wasm32"))]
impl std::fmt::Display for PreparedSolveCacheWriteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLimits(error) => formatter.write_str(error),
            Self::InvalidPath => {
                formatter.write_str("publication is outside the owned v5 cache namespace")
            }
            Self::Codec(error) => write!(formatter, "cache serialization rejected: {error}"),
            Self::Compression(error) => write!(formatter, "cache compression rejected: {error}"),
            Self::Storage(error) => {
                write!(formatter, "cache publication/retention failed: {error}")
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl From<StorageError> for PreparedSolveCacheWriteError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

/// Check a stream's byte budget before delegating any write or allocation.
#[cfg(not(target_arch = "wasm32"))]
struct BudgetWriter<W> {
    inner: W,
    used: usize,
    limit: usize,
    label: &'static str,
}

#[cfg(not(target_arch = "wasm32"))]
impl<W> BudgetWriter<W> {
    fn new(inner: W, limit: usize, label: &'static str) -> Self {
        Self {
            inner,
            used: 0,
            limit,
            label,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl<W: std::io::Write> std::io::Write for BudgetWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit - self.used {
            return Err(std::io::Error::other(format!(
                "{} byte budget {} exceeded",
                self.label, self.limit,
            )));
        }
        let written = self.inner.write(bytes)?;
        self.used += written;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Reserve only bytes accepted by the outer compressed-byte budget.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct CacheBuffer(Vec<u8>);

#[cfg(not(target_arch = "wasm32"))]
impl std::io::Write for CacheBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .try_reserve_exact(bytes.len())
            .map_err(std::io::Error::other)?;
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl std::fmt::Display for PreparedSolveCacheReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLimits(error) => formatter.write_str(error),
            Self::Storage(error) => write!(formatter, "cache storage read rejected: {error}"),
            Self::Decompression(error) => {
                write!(formatter, "cache decompression rejected: {error}")
            }
            Self::DecodedLimit { max_bytes } => {
                write!(formatter, "decoded cache exceeds {max_bytes} bytes")
            }
            Self::Codec(error) => write!(formatter, "cache record decode rejected: {error}"),
            Self::TrailingBytes => formatter.write_str("cache record has trailing bytes"),
            Self::Identity => {
                formatter.write_str("cache record identity does not match the admitted solve key")
            }
        }
    }
}

/// A prepared solve model is reusable for the exact structural source key,
/// admitted library revision, solver, and parameter override vector that
/// produced it. This is the same identity used by the persistent cache, so
/// equivalent generated networks share pure solve IR within one session too.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) struct PreparedSolveKey {
    pub(super) source_key: u64,
    pub(super) library_revision: u64,
    pub(super) solver_id: String,
    pub(super) parameter_overrides: Vec<(String, u64)>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Deserialize, Serialize)]
struct PreparedSolveDiskRecord<M, S, P> {
    version: u32,
    source_key: u64,
    library_revision: u64,
    solver_id: S,
    parameter_overrides: P,
    model: M,
}

pub(super) struct PreparedSolveCache {
    models: BoundedReuseCache<PreparedSolveKey, Arc<rumoca_ir_solve::SolveModel>>,
}

impl PreparedSolveCache {
    pub(super) fn new(capacity: NonZeroUsize) -> Self {
        Self {
            models: BoundedReuseCache::new(capacity),
        }
    }

    pub(super) fn contains_key(&self, key: &PreparedSolveKey) -> bool {
        self.models.contains_key(key)
    }

    pub(super) fn get(&self, key: &PreparedSolveKey) -> Option<&Arc<rumoca_ir_solve::SolveModel>> {
        self.models.get(key)
    }

    pub(super) fn insert(
        &mut self,
        key: PreparedSolveKey,
        model: Arc<rumoca_ir_solve::SolveModel>,
    ) {
        self.models.insert(key, model);
    }

    pub(super) fn key(
        source_key: u64,
        library_revision: u64,
        spec: &solver::SolverSpec,
        parameter_overrides: &[(String, f64)],
    ) -> PreparedSolveKey {
        PreparedSolveKey {
            source_key,
            library_revision,
            solver_id: spec.id.to_string(),
            parameter_overrides: parameter_overrides
                .iter()
                .map(|(name, value)| (name.clone(), value.to_bits()))
                .collect(),
        }
    }

    pub(super) fn clear(&mut self) {
        self.models.clear();
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn disk_filename(key: &PreparedSolveKey) -> String {
        use std::hash::{Hash, Hasher};

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        PREPARED_SOLVE_CACHE_VERSION.hash(&mut hasher);
        key.source_key.hash(&mut hasher);
        key.library_revision.hash(&mut hasher);
        key.solver_id.hash(&mut hasher);
        key.parameter_overrides.hash(&mut hasher);
        let hash = hasher.finish();
        format!("{:016x}-{hash:016x}.bin.zst", key.source_key)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn disk_path(key: &PreparedSolveKey) -> std::path::PathBuf {
        modelica_dir()
            .join("prepared-solve-v5")
            .join(Self::disk_filename(key))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn load_disk(
        key: &PreparedSolveKey,
        limits: PreparedSolveDiskLimits,
    ) -> Result<Option<rumoca_ir_solve::SolveModel>, PreparedSolveCacheReadError> {
        let path = Self::disk_path(key);
        Self::load_disk_at(&FileStorage::new(), &StorageHandle::File(path), key, limits)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn load_disk_at(
        storage: &FileStorage,
        handle: &StorageHandle,
        key: &PreparedSolveKey,
        limits: PreparedSolveDiskLimits,
    ) -> Result<Option<rumoca_ir_solve::SolveModel>, PreparedSolveCacheReadError> {
        use std::io::Read;

        limits
            .validate()
            .map_err(PreparedSolveCacheReadError::InvalidLimits)?;
        let compressed =
            match bevy::tasks::block_on(storage.read_bounded(handle, limits.compressed_bytes)) {
                Ok(bytes) => bytes,
                Err(StorageError::NotFound) => return Ok(None),
                Err(error) => return Err(PreparedSolveCacheReadError::Storage(error)),
            };
        let mut decoder = zstd::stream::read::Decoder::with_buffer(compressed.as_slice())
            .map_err(PreparedSolveCacheReadError::Decompression)?;
        decoder
            .window_log_max(limits.zstd_window_log_max)
            .map_err(PreparedSolveCacheReadError::Decompression)?;
        let mut bytes = Vec::new();
        // A sentinel byte rejects an oversized stream without materializing it.
        decoder
            .take(limits.decoded_bytes as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(PreparedSolveCacheReadError::Decompression)?;
        if bytes.len() > limits.decoded_bytes {
            return Err(PreparedSolveCacheReadError::DecodedLimit {
                max_bytes: limits.decoded_bytes,
            });
        }
        let (record, consumed): (
            PreparedSolveDiskRecord<rumoca_ir_solve::SolveModel, String, Vec<(String, u64)>>,
            usize,
        ) = bincode::serde::decode_from_slice(
            &bytes,
            bincode::config::standard()
                .with_limit::<{ PreparedSolveDiskLimits::CODEC_MAX_DECODED_BYTES }>(),
        )
        .map_err(PreparedSolveCacheReadError::Codec)?;
        if consumed != bytes.len() {
            return Err(PreparedSolveCacheReadError::TrailingBytes);
        }
        if record.version != PREPARED_SOLVE_CACHE_VERSION
            || record.source_key != key.source_key
            || record.library_revision != key.library_revision
            || record.solver_id != key.solver_id
            || record.parameter_overrides != key.parameter_overrides
        {
            return Err(PreparedSolveCacheReadError::Identity);
        }
        // Recency is retention metadata, not cache validity: a failed mark
        // keeps the verified model and only ages this record sooner.
        if let Err(error) = storage.mark_cache_file_used(handle) {
            log::warn!("[modelica-runtime] prepared-solve cache use was not recorded: {error}");
        }
        Ok(Some(record.model))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn save_disk(
        key: &PreparedSolveKey,
        model: &rumoca_ir_solve::SolveModel,
        limits: PreparedSolveDiskLimits,
    ) -> Result<(), PreparedSolveCacheWriteError> {
        let path = Self::disk_path(key);
        let root = path
            .parent()
            .ok_or(PreparedSolveCacheWriteError::InvalidPath)?;
        Self::save_disk_at(&FileStorage::new(), root, &path, key, model, limits)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn encode_disk(
        key: &PreparedSolveKey,
        model: &rumoca_ir_solve::SolveModel,
        limits: PreparedSolveDiskLimits,
    ) -> Result<Vec<u8>, PreparedSolveCacheWriteError> {
        limits
            .validate()
            .map_err(PreparedSolveCacheWriteError::InvalidLimits)?;
        let record = PreparedSolveDiskRecord {
            version: PREPARED_SOLVE_CACHE_VERSION,
            source_key: key.source_key,
            library_revision: key.library_revision,
            solver_id: key.solver_id.as_str(),
            parameter_overrides: key.parameter_overrides.as_slice(),
            model,
        };
        let compressed = BudgetWriter::new(
            CacheBuffer::default(),
            limits.compressed_bytes,
            "compressed",
        );
        let mut encoder = zstd::stream::write::Encoder::new(compressed, 3)
            .map_err(PreparedSolveCacheWriteError::Compression)?;
        encoder
            .window_log(limits.zstd_window_log_max)
            .map_err(PreparedSolveCacheWriteError::Compression)?;
        let mut decoded = BudgetWriter::new(encoder, limits.decoded_bytes, "decoded");
        bincode::serde::encode_into_std_write(record, &mut decoded, bincode::config::standard())
            .map_err(PreparedSolveCacheWriteError::Codec)?;
        let compressed = decoded
            .inner
            .finish()
            .map_err(PreparedSolveCacheWriteError::Compression)?;
        Ok(compressed.inner.0)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn save_disk_at(
        storage: &FileStorage,
        root: &std::path::Path,
        path: &std::path::Path,
        key: &PreparedSolveKey,
        model: &rumoca_ir_solve::SolveModel,
        limits: PreparedSolveDiskLimits,
    ) -> Result<(), PreparedSolveCacheWriteError> {
        if path.parent() != Some(root)
            || path.file_name().and_then(|name| name.to_str())
                != Some(Self::disk_filename(key).as_str())
        {
            return Err(PreparedSolveCacheWriteError::InvalidPath);
        }
        // Serialization/compression do not hold the cross-process publication lock.
        let compressed = Self::encode_disk(key, model, limits)?;
        let transaction = storage.lock_cache_directory(&StorageHandle::File(root.to_path_buf()))?;
        let incoming = transaction.directory().join(
            path.file_name()
                .ok_or(PreparedSolveCacheWriteError::InvalidPath)?,
        );
        Self::retain_disk(storage, &transaction, &incoming, limits)?;
        storage.write_sync(&StorageHandle::File(incoming), &compressed)?;
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn retain_disk(
        storage: &FileStorage,
        transaction: &lunco_storage::file_storage::DirectoryCacheTransaction,
        incoming: &std::path::Path,
        limits: PreparedSolveDiskLimits,
    ) -> Result<(), PreparedSolveCacheWriteError> {
        use std::collections::BTreeSet;
        // Keep the N-1 most recently published or used other valid-size
        // records; loads mark hits as used. Ties break by filename, so the
        // choice is independent of OS enumeration order.
        let mut recent = BTreeSet::new();
        for file in transaction.files()? {
            let file = file?;
            let StorageHandle::File(path) = file.handle else {
                return Err(PreparedSolveCacheWriteError::InvalidPath);
            };
            if path != incoming
                && owned_cache_filename(&path)
                && file.bytes <= limits.compressed_bytes as u64
            {
                recent.insert((std::cmp::Reverse(file.modified), path));
                if recent.len() >= limits.retained_entries {
                    recent.pop_last();
                }
            }
        }
        let keep: BTreeSet<_> = recent.into_iter().map(|(_, path)| path).collect();
        loop {
            // A bounded sorted batch avoids modifying a directory while its
            // enumerator is live. Each pass makes progress through actual deletes.
            let mut victims = BTreeSet::new();
            for file in transaction.files()? {
                let file = file?;
                let StorageHandle::File(path) = file.handle else {
                    return Err(PreparedSolveCacheWriteError::InvalidPath);
                };
                if path != incoming && owned_cache_filename(&path) && !keep.contains(&path) {
                    victims.insert(path);
                    if victims.len() > limits.retained_entries {
                        victims.pop_first();
                    }
                }
            }
            if victims.is_empty() {
                break;
            }
            for path in victims.into_iter().rev() {
                storage.delete_sync(&StorageHandle::File(path))?;
            }
        }
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn owned_cache_filename(path: &std::path::Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(key) = name.strip_suffix(".bin.zst") else {
        return false;
    };
    let bytes = key.as_bytes();
    bytes.len() == 33
        && bytes[16] == b'-'
        && bytes.iter().enumerate().all(|(index, byte)| {
            index == 16 || byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)
        })
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use lunco_storage::Storage;

    fn key() -> PreparedSolveKey {
        PreparedSolveKey {
            source_key: 11,
            library_revision: 22,
            solver_id: "generic-solver".into(),
            parameter_overrides: vec![("gain".into(), 3.5_f64.to_bits())],
        }
    }

    fn record(
        key: &PreparedSolveKey,
    ) -> PreparedSolveDiskRecord<rumoca_ir_solve::SolveModel, String, Vec<(String, u64)>> {
        PreparedSolveDiskRecord {
            version: PREPARED_SOLVE_CACHE_VERSION,
            source_key: key.source_key,
            library_revision: key.library_revision,
            solver_id: key.solver_id.clone(),
            parameter_overrides: key.parameter_overrides.clone(),
            model: rumoca_ir_solve::SolveModel {
                initial_y: vec![42.0],
                parameters: vec![3.5],
                ..Default::default()
            },
        }
    }

    fn encode(
        record: &PreparedSolveDiskRecord<rumoca_ir_solve::SolveModel, String, Vec<(String, u64)>>,
    ) -> Vec<u8> {
        bincode::serde::encode_to_vec(record, bincode::config::standard()).unwrap()
    }

    fn compress(bytes: &[u8]) -> Vec<u8> {
        zstd::stream::encode_all(bytes, 3).unwrap()
    }

    fn stored(bytes: &[u8]) -> (FileStorage, StorageHandle) {
        let storage = FileStorage::new();
        let handle = StorageHandle::Memory("generic-solve-cache".into());
        storage.write_sync(&handle, bytes).unwrap();
        (storage, handle)
    }

    #[test]
    fn persistent_solve_cache_streams_borrowed_record_and_bounds_both_byte_budgets() {
        use std::io::Write;
        let key = key();
        let record = record(&key);
        let decoded_len = encode(&record).len();
        let limits = PreparedSolveDiskLimits {
            decoded_bytes: decoded_len,
            ..Default::default()
        };
        let compressed = PreparedSolveCache::encode_disk(&key, &record.model, limits).unwrap();
        let exact = PreparedSolveDiskLimits {
            compressed_bytes: compressed.len(),
            ..limits
        };
        assert_eq!(
            PreparedSolveCache::encode_disk(&key, &record.model, exact).unwrap(),
            compressed
        );
        let (storage, handle) = stored(&compressed);
        let model = PreparedSolveCache::load_disk_at(&storage, &handle, &key, exact)
            .unwrap()
            .unwrap();
        assert_eq!(model.initial_y, record.model.initial_y);
        assert_eq!(model.parameters, record.model.parameters);
        let decoded_error = PreparedSolveCache::encode_disk(
            &key,
            &record.model,
            PreparedSolveDiskLimits {
                decoded_bytes: decoded_len - 1,
                ..exact
            },
        )
        .unwrap_err();
        assert!(decoded_error.to_string().contains("decoded byte budget"));
        let compressed_error = PreparedSolveCache::encode_disk(
            &key,
            &record.model,
            PreparedSolveDiskLimits {
                compressed_bytes: 1,
                ..exact
            },
        )
        .unwrap_err();
        assert!(
            compressed_error
                .to_string()
                .contains("compressed byte budget")
        );
        let mut writer = BudgetWriter::new(CacheBuffer::default(), 2, "test");
        assert!(writer.write_all(b"abc").is_err());
        assert_eq!(writer.used, 0);
        assert!(writer.inner.0.is_empty());
        assert_eq!(writer.inner.0.capacity(), 0);
        writer.write_all(b"ab").unwrap();
        assert_eq!(writer.inner.0, b"ab");
    }

    #[test]
    fn persistent_solve_cache_publication_retains_only_owned_regular_files() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("cache é");
        let storage = FileStorage::new();
        let limits = PreparedSolveDiskLimits {
            retained_entries: 2,
            compressed_bytes: 512,
            ..Default::default()
        };
        for source in 1..=8_u64 {
            let name = format!("{source:016x}-0000000000000000.bin.zst");
            storage
                .write_sync(&StorageHandle::File(directory.join(name)), b"old")
                .unwrap();
        }
        // A later recorded use makes source 3 the most recent pre-existing record.
        std::thread::sleep(std::time::Duration::from_millis(20));
        storage
            .mark_cache_file_used(&StorageHandle::File(
                directory.join("0000000000000003-0000000000000000.bin.zst"),
            ))
            .unwrap();
        let oversized = directory.join("0000000000000000-0000000000000000.bin.zst");
        storage
            .write_sync(
                &StorageHandle::File(oversized.clone()),
                &vec![0; limits.compressed_bytes + 1],
            )
            .unwrap();
        let unrelated = StorageHandle::File(directory.join("settings.json"));
        storage.write_sync(&unrelated, b"user preferences").unwrap();
        let malformed =
            StorageHandle::File(directory.join("FFFFFFFFFFFFFFFF-0000000000000000.bin.zst"));
        storage.write_sync(&malformed, b"unrecognized").unwrap();
        let nested =
            StorageHandle::File(directory.join("0000000000000000-0000000000000001.bin.zst"));
        storage.ensure_directory_sync(&nested).unwrap();
        #[cfg(unix)]
        let link = directory.join("0000000000000000-0000000000000002.bin.zst");
        #[cfg(unix)]
        lunco_storage::create_file_symlink_sync(&directory.join("settings.json"), &link).unwrap();
        let mut key = key();
        key.source_key = 100;
        let incoming = directory.join(PreparedSolveCache::disk_filename(&key));
        let model = record(&key).model;
        PreparedSolveCache::save_disk_at(&storage, &directory, &incoming, &key, &model, limits)
            .unwrap();
        let transaction = storage
            .lock_cache_directory(&StorageHandle::File(directory.clone()))
            .unwrap();
        let retained = transaction
            .files()
            .unwrap()
            .filter_map(|entry| {
                let file = entry.unwrap();
                let StorageHandle::File(path) = file.handle else {
                    panic!("native file");
                };
                owned_cache_filename(&path).then_some(path)
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(retained.len(), 2);
        assert!(
            retained.contains(
                &transaction
                    .directory()
                    .join("0000000000000003-0000000000000000.bin.zst")
            ),
            "the most recently used record survives regardless of filename order"
        );
        assert!(retained.contains(&transaction.directory().join(incoming.file_name().unwrap())));
        assert!(!bevy::tasks::block_on(
            storage.exists(&StorageHandle::File(oversized))
        ));
        assert_eq!(storage.read_sync(&unrelated).unwrap(), b"user preferences");
        assert_eq!(storage.read_sync(&malformed).unwrap(), b"unrecognized");
        assert_eq!(
            storage.entry_kind_sync(&nested).unwrap(),
            lunco_storage::StorageEntryKind::Directory
        );
        #[cfg(unix)]
        assert_eq!(
            storage.read_sync(&StorageHandle::File(link)).unwrap(),
            b"user preferences"
        );
        drop(transaction);
        let loaded = PreparedSolveCache::load_disk_at(
            &storage,
            &StorageHandle::File(incoming),
            &key,
            limits,
        )
        .unwrap()
        .unwrap();
        assert_eq!(loaded.initial_y, model.initial_y);
    }

    #[test]
    fn persistent_solve_cache_concurrent_publications_enforce_exact_directory_quota() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("shared cache");
        let limits = PreparedSolveDiskLimits {
            retained_entries: 3,
            compressed_bytes: 1024,
            ..Default::default()
        };
        std::thread::scope(|scope| {
            for source in 0..8 {
                let directory = &directory;
                scope.spawn(move || {
                    let storage = FileStorage::new();
                    for revision in 0..8 {
                        let mut key = key();
                        key.source_key = source;
                        key.library_revision = revision;
                        let model = record(&key).model;
                        let path = directory.join(PreparedSolveCache::disk_filename(&key));
                        PreparedSolveCache::save_disk_at(
                            &storage, directory, &path, &key, &model, limits,
                        )
                        .unwrap();
                    }
                });
            }
        });
        let storage = FileStorage::new();
        let transaction = storage
            .lock_cache_directory(&StorageHandle::File(directory.clone()))
            .unwrap();
        let owned = transaction
            .files()
            .unwrap()
            .filter_map(|entry| {
                let file = entry.unwrap();
                let StorageHandle::File(path) = file.handle else {
                    panic!("native file");
                };
                owned_cache_filename(&path).then_some((path, file.bytes))
            })
            .collect::<Vec<_>>();
        assert_eq!(owned.len(), limits.retained_entries);
        assert!(
            owned
                .iter()
                .all(|(_, bytes)| *bytes <= limits.compressed_bytes as u64)
        );
        for (path, _) in owned {
            let mut matching_keys = 0;
            for source in 0..8 {
                for revision in 0..8 {
                    let mut key = key();
                    key.source_key = source;
                    key.library_revision = revision;
                    if path.file_name().unwrap()
                        == std::ffi::OsStr::new(&PreparedSolveCache::disk_filename(&key))
                    {
                        let loaded = PreparedSolveCache::load_disk_at(
                            &storage,
                            &StorageHandle::File(path.clone()),
                            &key,
                            limits,
                        )
                        .unwrap()
                        .unwrap();
                        assert_eq!(loaded.initial_y, [42.0]);
                        matching_keys += 1;
                    }
                }
            }
            assert_eq!(matching_keys, 1);
        }
    }

    #[test]
    fn persistent_solve_cache_rejects_invalid_limits_paths_and_storage_failure() {
        let root = tempfile::tempdir().unwrap();
        let storage = FileStorage::new();
        let key = key();
        let model = record(&key).model;
        let path = root.path().join(PreparedSolveCache::disk_filename(&key));
        let invalid = PreparedSolveDiskLimits {
            retained_entries: 0,
            ..Default::default()
        };
        assert!(matches!(
            PreparedSolveCache::save_disk_at(&storage, root.path(), &path, &key, &model, invalid),
            Err(PreparedSolveCacheWriteError::InvalidLimits(_))
        ));
        assert_eq!(PreparedSolveDiskLimits::default().retained_entries, 32);
        let (_command_tx, command_rx) = crossbeam_channel::unbounded();
        let (result_tx, result_rx) = crossbeam_channel::unbounded();
        super::super::modelica_worker(
            command_rx,
            result_tx,
            invalid,
            lunco_modelica_runtime::ModelicaCacheLimits::default(),
        );
        assert!(
            result_rx
                .try_recv()
                .unwrap()
                .worker_failure
                .unwrap()
                .contains("retained entries must be positive")
        );
        assert!(matches!(
            PreparedSolveCache::save_disk_at(
                &storage,
                root.path(),
                &root.path().join("not-owned.bin.zst"),
                &key,
                &model,
                Default::default()
            ),
            Err(PreparedSolveCacheWriteError::InvalidPath)
        ));
        let outside = root
            .path()
            .join("other-directory")
            .join(path.file_name().unwrap());
        assert!(matches!(
            PreparedSolveCache::save_disk_at(
                &storage,
                root.path(),
                &outside,
                &key,
                &model,
                Default::default()
            ),
            Err(PreparedSolveCacheWriteError::InvalidPath)
        ));
        let blocked = root.path().join("file-is-not-a-directory");
        storage
            .write_sync(&StorageHandle::File(blocked.clone()), b"preserve")
            .unwrap();
        let path = blocked.join(PreparedSolveCache::disk_filename(&key));
        let error = PreparedSolveCache::save_disk_at(
            &storage,
            &blocked,
            &path,
            &key,
            &model,
            Default::default(),
        )
        .unwrap_err();
        assert!(matches!(error, PreparedSolveCacheWriteError::Storage(_)));
        assert!(error.to_string().contains("publication/retention failed"));
        assert_eq!(
            storage.read_sync(&StorageHandle::File(blocked)).unwrap(),
            b"preserve"
        );
    }

    #[test]
    fn prepared_solve_disk_cache_accepts_exact_budgets_and_missing_record() {
        let key = key();
        let encoded = encode(&record(&key));
        let compressed = compress(&encoded);
        let (storage, handle) = stored(&compressed);
        let limits = PreparedSolveDiskLimits {
            compressed_bytes: compressed.len(),
            decoded_bytes: encoded.len(),
            ..Default::default()
        };
        let model = PreparedSolveCache::load_disk_at(&storage, &handle, &key, limits)
            .unwrap()
            .unwrap();
        assert_eq!(model.initial_y, [42.0]);
        assert_eq!(model.parameters, [3.5]);
        assert!(
            PreparedSolveCache::load_disk_at(
                &storage,
                &StorageHandle::Memory("absent-cache".into()),
                &key,
                limits,
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn prepared_solve_disk_cache_rejects_storage_and_expansion_over_budget() {
        let key = key();
        let encoded = encode(&record(&key));
        let compressed = compress(&encoded);
        let (storage, handle) = stored(&compressed);
        assert!(matches!(
            PreparedSolveCache::load_disk_at(
                &storage,
                &handle,
                &key,
                PreparedSolveDiskLimits {
                    compressed_bytes: compressed.len() - 1,
                    ..Default::default()
                },
            ),
            Err(PreparedSolveCacheReadError::Storage(
                StorageError::SizeLimitExceeded { .. }
            ))
        ));
        assert!(matches!(
            PreparedSolveCache::load_disk_at(
                &storage,
                &handle,
                &key,
                PreparedSolveDiskLimits {
                    decoded_bytes: encoded.len() - 1,
                    ..Default::default()
                },
            ),
            Err(PreparedSolveCacheReadError::DecodedLimit { .. })
        ));
        let (storage, handle) = stored(&compress(&vec![0; 1024 * 1024]));
        assert!(matches!(
            PreparedSolveCache::load_disk_at(
                &storage,
                &handle,
                &key,
                PreparedSolveDiskLimits {
                    decoded_bytes: 32,
                    ..Default::default()
                },
            ),
            Err(PreparedSolveCacheReadError::DecodedLimit { max_bytes: 32 })
        ));
    }

    #[test]
    fn prepared_solve_disk_cache_rejects_malformed_truncated_and_trailing_records() {
        let key = key();
        let encoded = encode(&record(&key));
        let compressed = compress(&encoded);
        for bytes in [
            b"invalid zstd".as_slice(),
            &compressed[..compressed.len() - 1],
        ] {
            let (storage, handle) = stored(bytes);
            assert!(matches!(
                PreparedSolveCache::load_disk_at(
                    &storage,
                    &handle,
                    &key,
                    PreparedSolveDiskLimits::default(),
                ),
                Err(PreparedSolveCacheReadError::Decompression(_))
            ));
        }
        let (storage, handle) = stored(&compress(&encoded[..encoded.len() - 1]));
        assert!(matches!(
            PreparedSolveCache::load_disk_at(
                &storage,
                &handle,
                &key,
                PreparedSolveDiskLimits::default(),
            ),
            Err(PreparedSolveCacheReadError::Codec(_))
        ));
        let mut trailing = encoded;
        trailing.push(0);
        let (storage, handle) = stored(&compress(&trailing));
        assert!(matches!(
            PreparedSolveCache::load_disk_at(
                &storage,
                &handle,
                &key,
                PreparedSolveDiskLimits::default(),
            ),
            Err(PreparedSolveCacheReadError::TrailingBytes)
        ));
    }

    #[test]
    fn prepared_solve_disk_cache_rejects_every_identity_mismatch() {
        let key = key();
        for field in 0..5 {
            let mut record = record(&key);
            match field {
                0 => record.version += 1,
                1 => record.source_key += 1,
                2 => record.library_revision += 1,
                3 => record.solver_id.push('x'),
                4 => record.parameter_overrides[0].1 = 7.0_f64.to_bits(),
                _ => unreachable!(),
            }
            let (storage, handle) = stored(&compress(&encode(&record)));
            assert!(
                matches!(
                    PreparedSolveCache::load_disk_at(
                        &storage,
                        &handle,
                        &key,
                        PreparedSolveDiskLimits::default(),
                    ),
                    Err(PreparedSolveCacheReadError::Identity)
                ),
                "field {field}"
            );
        }
    }

    #[test]
    fn prepared_solve_disk_cache_rejects_excessive_zstd_window() {
        use std::io::Write;
        let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 3).unwrap();
        encoder.window_log(20).unwrap();
        encoder.write_all(&vec![0; 64 * 1024]).unwrap();
        let (storage, handle) = stored(&encoder.finish().unwrap());
        assert!(matches!(
            PreparedSolveCache::load_disk_at(
                &storage,
                &handle,
                &key(),
                PreparedSolveDiskLimits {
                    zstd_window_log_max: 10,
                    ..Default::default()
                },
            ),
            Err(PreparedSolveCacheReadError::Decompression(_))
        ));
    }

    #[test]
    fn prepared_solve_disk_cache_rejects_invalid_limits_before_worker_start() {
        let invalid = PreparedSolveDiskLimits {
            decoded_bytes: 0,
            ..Default::default()
        };
        let (_command_tx, command_rx) = crossbeam_channel::unbounded();
        let (result_tx, result_rx) = crossbeam_channel::unbounded();
        super::super::modelica_worker(
            command_rx,
            result_tx,
            invalid,
            lunco_modelica_runtime::ModelicaCacheLimits::default(),
        );
        let result = result_rx.try_recv().unwrap();
        assert!(
            result
                .worker_failure
                .as_deref()
                .unwrap()
                .contains("invalid prepared-solve disk-cache limits")
        );
        for limits in [
            PreparedSolveDiskLimits {
                compressed_bytes: 0,
                ..Default::default()
            },
            PreparedSolveDiskLimits {
                decoded_bytes: PreparedSolveDiskLimits::CODEC_MAX_DECODED_BYTES + 1,
                ..Default::default()
            },
            PreparedSolveDiskLimits {
                zstd_window_log_max: 9,
                ..Default::default()
            },
            PreparedSolveDiskLimits {
                zstd_window_log_max: PreparedSolveDiskLimits::MAX_WINDOW_LOG + 1,
                ..Default::default()
            },
        ] {
            assert!(limits.validate().is_err());
        }
    }
}

#[cfg(test)]
mod reuse_tests {
    use super::*;
    use lunco_modelica_runtime::ModelicaCacheLimits;

    #[test]
    fn immutable_reuse_cache_bounds_entries_and_preserves_fifo_on_reads_and_replacement() {
        let mut cache = BoundedReuseCache::new(NonZeroUsize::new(2).unwrap());
        cache.insert(1, "first");
        cache.insert(2, "second");
        assert_eq!(cache.get(&1), Some(&"first"));
        cache.insert(1, "replacement");
        assert_eq!(cache.len(), 2);
        cache.insert(3, "third");
        assert!(!cache.contains_key(&1));
        assert_eq!(cache.get(&2), Some(&"second"));
        assert_eq!(cache.get(&3), Some(&"third"));
        for key in 4..1000 {
            cache.insert(key, "new graph");
        }
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.admission_order.len(), 2);
        assert!(cache.contains_key(&998));
        assert!(cache.contains_key(&999));
        cache.clear();
        assert_eq!(cache.len(), 0);
        assert!(cache.admission_order.is_empty());
        cache.insert(7, "fresh admission");
        cache.insert(8, "second admission");
        cache.insert(9, "third admission");
        assert!(!cache.contains_key(&7));
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn immutable_reuse_cache_eviction_preserves_live_arc_without_copying_graph() {
        let mut cache = BoundedReuseCache::new(NonZeroUsize::new(1).unwrap());
        let graph = Arc::new(vec![1_u64, 2, 3]);
        let weak = Arc::downgrade(&graph);
        cache.insert(1, graph);
        let live = Arc::clone(cache.get(&1).unwrap());
        assert_eq!(Arc::strong_count(&live), 2);
        cache.insert(2, Arc::new(vec![4]));
        assert_eq!(Arc::strong_count(&live), 1);
        assert_eq!(&*live, &[1, 2, 3]);
        assert!(weak.upgrade().is_some());
        drop(live);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn immutable_reuse_cache_keeps_exact_solve_key_and_shared_model_identity() {
        let mut cache = PreparedSolveCache::new(NonZeroUsize::new(2).unwrap());
        let key = PreparedSolveKey {
            source_key: 1,
            library_revision: 2,
            solver_id: "generic-solver".into(),
            parameter_overrides: vec![("gain".into(), 3.0_f64.to_bits())],
        };
        let graph = Arc::new(rumoca_ir_solve::SolveModel::default());
        cache.insert(key.clone(), Arc::clone(&graph));
        assert!(Arc::ptr_eq(cache.get(&key).unwrap(), &graph));
        let mut parameter_key = key.clone();
        parameter_key.parameter_overrides[0].1 = 4.0_f64.to_bits();
        assert!(!cache.contains_key(&parameter_key));
        cache.insert(parameter_key.clone(), Arc::clone(&graph));
        let mut library_key = key.clone();
        library_key.library_revision += 1;
        cache.insert(library_key.clone(), Arc::clone(&graph));
        assert!(!cache.contains_key(&key));
        assert!(cache.contains_key(&parameter_key));
        assert!(cache.contains_key(&library_key));
        cache.clear();
        assert!(!cache.contains_key(&library_key));
        assert_eq!(Arc::strong_count(&graph), 1);
    }

    #[test]
    fn immutable_reuse_cache_limits_reject_zero_capacity() {
        for limits in [
            ModelicaCacheLimits {
                compiled_artifact_entries: 0,
                ..Default::default()
            },
            ModelicaCacheLimits {
                prepared_solve_entries: 0,
                ..Default::default()
            },
        ] {
            assert!(limits.validate().is_err());
        }
        let (compiled, prepared) = ModelicaCacheLimits::default().validate().unwrap();
        assert_eq!(compiled.get(), 64);
        assert_eq!(prepared.get(), 64);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn immutable_reuse_cache_limits_fail_worker_before_thread_admission() {
        let (_command_tx, command_rx) = crossbeam_channel::unbounded();
        let (result_tx, result_rx) = crossbeam_channel::unbounded();
        super::super::modelica_worker(
            command_rx,
            result_tx,
            PreparedSolveDiskLimits::default(),
            ModelicaCacheLimits {
                compiled_artifact_entries: 0,
                ..Default::default()
            },
        );
        let result = result_rx.try_recv().unwrap();
        assert!(
            result
                .worker_failure
                .as_deref()
                .unwrap()
                .contains("capacity must be positive")
        );
        assert!(result.error.is_none());
    }
}
