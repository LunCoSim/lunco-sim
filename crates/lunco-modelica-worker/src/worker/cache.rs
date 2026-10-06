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
use lunco_storage::{FileStorage, StorageError, StorageHandle, write_file_sync};
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

/// Native optional solve-cache read budgets, captured when the worker starts.
/// Insert this resource before `ModelicaExecutionPlugin` to change the budgets.
/// The codec ceiling is an admission invariant; a larger artifact is recomputed
/// from its admitted DAE instead of being loaded from optional storage.
#[cfg(not(target_arch = "wasm32"))]
#[derive(bevy::prelude::Resource, Clone, Copy, Debug)]
pub struct PreparedSolveDiskLimits {
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
        if self.compressed_bytes == 0
            || self.compressed_bytes > Self::CODEC_MAX_DECODED_BYTES
            || self.decoded_bytes == 0
            || self.decoded_bytes > Self::CODEC_MAX_DECODED_BYTES
            || !(10..=Self::MAX_WINDOW_LOG).contains(&self.zstd_window_log_max)
        {
            return Err(format!(
                "invalid prepared-solve disk-cache limits: compressed/decoded byte budgets must be 1..={} and zstd window log must be 10..={}",
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
struct PreparedSolveDiskRecord {
    version: u32,
    source_key: u64,
    library_revision: u64,
    solver_id: String,
    parameter_overrides: Vec<(String, u64)>,
    model: rumoca_ir_solve::SolveModel,
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
    fn disk_path(
        source_key: u64,
        library_revision: u64,
        solver_id: &str,
        parameter_overrides: &[(String, u64)],
    ) -> std::path::PathBuf {
        use std::hash::{Hash, Hasher};

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        PREPARED_SOLVE_CACHE_VERSION.hash(&mut hasher);
        source_key.hash(&mut hasher);
        library_revision.hash(&mut hasher);
        solver_id.hash(&mut hasher);
        parameter_overrides.hash(&mut hasher);
        let key = hasher.finish();
        modelica_dir()
            .join("prepared-solve-v5")
            .join(format!("{source_key:016x}-{key:016x}.bin.zst"))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn load_disk(
        key: &PreparedSolveKey,
        limits: PreparedSolveDiskLimits,
    ) -> Result<Option<rumoca_ir_solve::SolveModel>, PreparedSolveCacheReadError> {
        let path = Self::disk_path(
            key.source_key,
            key.library_revision,
            &key.solver_id,
            &key.parameter_overrides,
        );
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
        let (record, consumed): (PreparedSolveDiskRecord, usize) =
            bincode::serde::decode_from_slice(
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
        Ok(Some(record.model))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn save_disk(
        source_key: u64,
        library_revision: u64,
        solver_id: &str,
        parameter_overrides: &[(String, u64)],
        model: &rumoca_ir_solve::SolveModel,
    ) {
        let path = Self::disk_path(source_key, library_revision, solver_id, parameter_overrides);
        let record = PreparedSolveDiskRecord {
            version: PREPARED_SOLVE_CACHE_VERSION,
            source_key,
            library_revision,
            solver_id: solver_id.to_owned(),
            parameter_overrides: parameter_overrides.to_vec(),
            model: model.clone(),
        };
        let Ok(bytes) = bincode::serde::encode_to_vec(record, bincode::config::standard()) else {
            return;
        };
        let Ok(compressed) = zstd::stream::encode_all(bytes.as_slice(), 3) else {
            return;
        };
        // Storage performs the native atomic replacement and owns the
        // platform-specific persistence path.
        let _ = write_file_sync(&path, &compressed);
    }
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

    fn record(key: &PreparedSolveKey) -> PreparedSolveDiskRecord {
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

    fn encode(record: &PreparedSolveDiskRecord) -> Vec<u8> {
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
