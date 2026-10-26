//! In-memory and persistent compiled-DAE and prepared-solve caches.
//!
//! Prepared graphs use participating-source content identity, solver identity,
//! and exact parameter and admitted input bits. Compiler admission revisions fence compiled
//! artifacts independently of immutable prepared-graph reuse.

#[cfg(not(target_arch = "wasm32"))]
use super::PREPARED_SOLVE_CACHE_VERSION;
use super::solver;
use linked_hash_map::LinkedHashMap;
#[cfg(not(target_arch = "wasm32"))]
use lunco_assets_core::modelica_dir;
#[cfg(not(target_arch = "wasm32"))]
use lunco_storage::{FileStorage, Storage, StorageError, StorageHandle};
#[cfg(not(target_arch = "wasm32"))]
use serde::{Deserialize, Serialize};
use std::hash::Hash;
use std::num::NonZeroUsize;
use std::sync::Arc;

/// Immutable strict compilation results. Persistent reuse is installed only
/// by the native compiler owner after its disk budgets have been admitted.
pub(super) struct CompiledArtifactCache {
    entries: BoundedReuseCache<u64, super::CompiledModelArtifact>,
    #[cfg(not(target_arch = "wasm32"))]
    disk_limits: Option<ModelicaArtifactDiskLimits>,
}

impl CompiledArtifactCache {
    pub(super) fn new(capacity: NonZeroUsize) -> Self {
        Self {
            entries: BoundedReuseCache::new(capacity),
            #[cfg(not(target_arch = "wasm32"))]
            disk_limits: None,
        }
    }

    pub(super) fn get(&mut self, key: &u64) -> Option<&super::CompiledModelArtifact> {
        self.entries.get(key)
    }

    pub(super) fn insert(&mut self, key: u64, artifact: super::CompiledModelArtifact) {
        self.entries.insert(key, artifact);
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
    }
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, PartialEq, Eq)]
pub(super) struct CompiledArtifactDiskKey {
    input: lunco_hash::content::Cid,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Serialize, Deserialize)]
struct CompiledArtifactDiskRecord<C> {
    version: u32,
    input: String,
    source_content: String,
    structural_overrides: Vec<(String, u64)>,
    compiled: C,
}

#[cfg(not(target_arch = "wasm32"))]
impl CompiledArtifactCache {
    const DISK_VERSION: u32 = 2;

    pub(super) fn enable_disk(&mut self, limits: ModelicaArtifactDiskLimits) {
        self.disk_limits = Some(limits);
    }

    pub(super) fn disk_key(
        &self,
        compiler: &super::ModelicaCompiler,
        model: &str,
        unit: &super::CompileUnit,
        uri: &str,
        numeric: &[(String, f64)],
    ) -> Result<Option<CompiledArtifactDiskKey>, String> {
        if self.disk_limits.is_none() {
            return Ok(None);
        }
        let Some(identity) =
            rumoca_core::build_identity().filter(|identity| !identity.ends_with("-dirty"))
        else {
            return Ok(None);
        };
        let context = compiler.admitted_compilation_identity()?;
        let mut hash = lunco_hash::content::ContentHasher::default();
        let mut field = |bytes: &[u8]| {
            hash.update(&(bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
        };
        field(b"LunCoStrictDaeArtifact");
        field(&Self::DISK_VERSION.to_le_bytes());
        field(identity.as_bytes());
        field(std::env::consts::ARCH.as_bytes());
        field(std::env::consts::OS.as_bytes());
        field(&context.to_bytes());
        field(model.as_bytes());
        field(uri.as_bytes());
        field(unit.source.as_bytes());
        field(&(unit.extras.len() as u64).to_le_bytes());
        for (uri, source) in &unit.extras {
            field(uri.as_bytes());
            field(source.as_bytes());
        }
        field(&(numeric.len() as u64).to_le_bytes());
        for (name, value) in numeric {
            field(name.as_bytes());
            field(&value.to_bits().to_le_bytes());
        }
        Ok(Some(CompiledArtifactDiskKey {
            input: hash.finish(),
        }))
    }

    fn disk_path(key: &CompiledArtifactDiskKey) -> std::path::PathBuf {
        let bytes = key.input.to_bytes();
        let first = lunco_hash::fnv1a64(&bytes);
        let second = lunco_hash::fnv1a64(key.input.to_string().as_bytes());
        modelica_dir()
            .join("compiled-dae-v2")
            .join(format!("{first:016x}-{second:016x}.bin.zst"))
    }

    pub(super) fn load_disk(
        &self,
        key: &CompiledArtifactDiskKey,
        numeric: &[(String, f64)],
    ) -> Result<Option<super::CompiledModelArtifact>, ModelicaCacheReadError> {
        let Some(limits) = self.disk_limits else {
            return Ok(None);
        };
        self.load_disk_at(
            &FileStorage::new(),
            &StorageHandle::File(Self::disk_path(key)),
            key,
            numeric,
            limits,
        )
    }

    fn load_disk_at(
        &self,
        storage: &FileStorage,
        handle: &StorageHandle,
        key: &CompiledArtifactDiskKey,
        numeric: &[(String, f64)],
        limits: ModelicaArtifactDiskLimits,
    ) -> Result<Option<super::CompiledModelArtifact>, ModelicaCacheReadError> {
        let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            read_record::<CompiledArtifactDiskRecord<Box<super::CompiledRuntimeModel>>>(
                storage, handle, limits,
            )
        }))
        .map_err(|_| {
            ModelicaCacheReadError::Codec(serde_json::Error::io(std::io::Error::other(
                "compiled artifact deserialization panicked",
            )))
        })??;
        let Some(record) = decoded else {
            return Ok(None);
        };
        if record.version != Self::DISK_VERSION
            || record.input != key.input.to_string()
            || record.structural_overrides
                != numeric
                    .iter()
                    .map(|(name, value)| (name.clone(), value.to_bits()))
                    .collect::<Vec<_>>()
        {
            return Err(ModelicaCacheReadError::Identity);
        }
        if !record.compiled.dae.external_tables().is_empty() {
            return Err(ModelicaCacheReadError::Identity);
        }
        let source_content = record
            .source_content
            .parse::<lunco_hash::content::Cid>()
            .map_err(|_| ModelicaCacheReadError::Identity)?;
        if let Err(error) = storage.mark_cache_file_used(handle) {
            log::warn!("[modelica-runtime] compiled-artifact cache use was not recorded: {error}");
        }
        Ok(Some(super::CompiledModelArtifact {
            compiled: record.compiled,
            prepared_source_key: super::prepared_source_key(source_content),
            structural_overrides: numeric.to_vec(),
            source_content,
        }))
    }

    pub(super) fn save_disk(
        &self,
        key: &CompiledArtifactDiskKey,
        artifact: &super::CompiledModelArtifact,
    ) -> Result<(), ModelicaCacheWriteError> {
        let Some(limits) = self.disk_limits else {
            return Ok(());
        };
        // Loaded external resources need their own captured byte identities;
        // source text alone cannot authorize reuse of their constructor data.
        if !artifact.compiled.dae.external_tables().is_empty() {
            log::debug!(
                "[modelica-runtime] compiled artifact is not source-closed: loaded external table data"
            );
            return Ok(());
        }
        let compressed = Self::encode_disk(key, artifact, limits)?;
        let path = Self::disk_path(key);
        let root = path.parent().ok_or(ModelicaCacheWriteError::InvalidPath)?;
        let storage = FileStorage::new();
        let transaction = storage.lock_cache_directory(&StorageHandle::File(root.to_path_buf()))?;
        let incoming = transaction.directory().join(
            path.file_name()
                .ok_or(ModelicaCacheWriteError::InvalidPath)?,
        );
        PreparedSolveCache::retain_disk(&storage, &transaction, &incoming, limits)?;
        storage.write_sync(&StorageHandle::File(incoming), &compressed)?;
        Ok(())
    }

    fn encode_disk(
        key: &CompiledArtifactDiskKey,
        artifact: &super::CompiledModelArtifact,
        limits: ModelicaArtifactDiskLimits,
    ) -> Result<Vec<u8>, ModelicaCacheWriteError> {
        if !artifact.compiled.dae.external_tables().is_empty() {
            return Err(ModelicaCacheWriteError::ConstructionWire(
                "compiled artifact has uncaptured external resource bytes".into(),
            ));
        }
        let record = CompiledArtifactDiskRecord {
            version: Self::DISK_VERSION,
            input: key.input.to_string(),
            source_content: artifact.source_content.to_string(),
            structural_overrides: artifact
                .structural_overrides
                .iter()
                .map(|(name, value)| (name.clone(), value.to_bits()))
                .collect(),
            compiled: &artifact.compiled,
        };
        encode_record(&record, limits)
    }
}

/// Count-bounded least-recently-used retention. Successful reads and
/// replacements refresh recency. Values are owned once and
/// borrowed on lookup, so eviction releases only the cache's reference.
pub(super) struct BoundedReuseCache<K, V> {
    entries: LinkedHashMap<K, V>,
    capacity: NonZeroUsize,
}

impl<K: Eq + Hash, V> BoundedReuseCache<K, V> {
    pub(super) fn new(capacity: NonZeroUsize) -> Self {
        Self {
            entries: LinkedHashMap::new(),
            capacity,
        }
    }

    pub(super) fn contains_key(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    pub(super) fn get(&mut self, key: &K) -> Option<&V> {
        self.entries.get_refresh(key).map(|value| &*value)
    }

    pub(super) fn insert(&mut self, key: K, value: V) -> Option<V> {
        let mut evicted = None;
        if self.entries.len() == self.capacity.get() && !self.entries.contains_key(&key) {
            evicted = self.pop_oldest();
        }
        self.entries.insert(key, value).or(evicted)
    }

    pub(super) fn pop_oldest(&mut self) -> Option<V> {
        self.entries.pop_front().map(|(_, value)| value)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn ordered_entries(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter()
    }

    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
    }
}

/// Optional Modelica artifact-cache byte and retention budgets captured at worker startup.
/// Insert this resource before `ModelicaExecutionPlugin` to change the budgets.
/// The codec ceiling is an admission invariant; a larger artifact is recomputed
/// from its admitted DAE instead of being loaded from optional storage.
#[cfg(not(target_arch = "wasm32"))]
#[derive(bevy::prelude::Resource, Clone, Copy, Debug)]
pub struct ModelicaArtifactDiskLimits {
    /// Maximum records per current-version artifact family after publication.
    pub retained_entries: usize,
    pub compressed_bytes: usize,
    pub decoded_bytes: usize,
    /// Maximum zstd back-reference window, expressed as a base-two logarithm.
    pub zstd_window_log_max: u32,
}

#[cfg(not(target_arch = "wasm32"))]
impl ModelicaArtifactDiskLimits {
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
                "invalid Modelica artifact disk-cache limits: retained entries must be positive, compressed/decoded byte budgets must be 1..={} and zstd window log must be 10..={}",
                Self::CODEC_MAX_DECODED_BYTES,
                Self::MAX_WINDOW_LOG,
            ));
        }
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for ModelicaArtifactDiskLimits {
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
pub(super) enum ModelicaCacheReadError {
    InvalidLimits(String),
    Storage(StorageError),
    Decompression(std::io::Error),
    DecodedLimit { max_bytes: usize },
    Codec(serde_json::Error),
    Identity,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub(super) enum ModelicaCacheWriteError {
    InvalidLimits(String),
    MissingCompilerIdentity,
    InvalidPath,
    ConstructionWire(String),
    Codec(serde_json::Error),
    BinaryCodec(bincode::error::EncodeError),
    Compression(std::io::Error),
    Storage(StorageError),
}

#[cfg(not(target_arch = "wasm32"))]
impl std::fmt::Display for ModelicaCacheWriteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLimits(error) => formatter.write_str(error),
            Self::MissingCompilerIdentity => formatter
                .write_str("persistent solver caching requires an identified clean compiler build"),
            Self::ConstructionWire(error) => write!(
                formatter,
                "checked component serialization rejected: {error}"
            ),
            Self::InvalidPath => {
                formatter.write_str("publication is outside its owned cache namespace")
            }
            Self::Codec(error) => write!(formatter, "cache serialization rejected: {error}"),
            Self::BinaryCodec(error) => write!(formatter, "cache serialization rejected: {error}"),
            Self::Compression(error) => write!(formatter, "cache compression rejected: {error}"),
            Self::Storage(error) => {
                write!(formatter, "cache publication/retention failed: {error}")
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl From<StorageError> for ModelicaCacheWriteError {
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
impl std::fmt::Display for ModelicaCacheReadError {
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
            Self::Identity => formatter.write_str("cache record identity does not match its owner"),
        }
    }
}

/// A prepared solve model is reusable for the exact structural source key,
/// solver, parameter overrides, and initial input vector that produced it. This is the same
/// identity used by the persistent cache, so equivalent generated networks
/// share pure solve IR within one session too.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) struct PreparedSolveKey {
    pub(super) compiler_identity: Option<&'static str>,
    pub(super) source_key: u64,
    pub(super) solver_id: String,
    pub(super) parameter_overrides: Vec<(String, u64)>,
    pub(super) initial_inputs: Vec<(String, u64)>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Serialize)]
struct PreparedSolveDiskRecord<M, S, P> {
    compiler_identity: Option<String>,
    version: u32,
    source_key: u64,
    solver_id: S,
    parameter_overrides: P,
    initial_inputs: P,
    model: M,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Deserialize)]
struct DecodedPreparedSolveDiskRecord {
    compiler_identity: Option<String>,
    version: u32,
    source_key: u64,
    solver_id: String,
    parameter_overrides: Vec<(String, u64)>,
    initial_inputs: Vec<(String, u64)>,
    #[serde(deserialize_with = "rumoca_sim::deserialize_fmi_component")]
    model: rumoca_ir_solve::fmi::FmiComponent,
}

pub(super) struct PreparedSolveCache {
    models: BoundedReuseCache<PreparedSolveKey, Arc<rumoca_ir_solve::fmi::FmiComponent>>,
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

    pub(super) fn get(
        &mut self,
        key: &PreparedSolveKey,
    ) -> Option<&Arc<rumoca_ir_solve::fmi::FmiComponent>> {
        self.models.get(key)
    }

    pub(super) fn insert(
        &mut self,
        key: PreparedSolveKey,
        model: Arc<rumoca_ir_solve::fmi::FmiComponent>,
    ) {
        self.models.insert(key, model);
    }

    pub(super) fn key(
        source_key: u64,
        spec: &solver::SolverSpec,
        parameter_overrides: &[(String, f64)],
        initial_inputs: &[(String, f64)],
    ) -> PreparedSolveKey {
        PreparedSolveKey {
            compiler_identity: rumoca_core::build_identity()
                .filter(|identity| !identity.ends_with("-dirty")),
            source_key,
            solver_id: spec.id.to_string(),
            parameter_overrides: parameter_overrides
                .iter()
                .map(|(name, value)| (name.clone(), value.to_bits()))
                .collect(),
            initial_inputs: initial_inputs
                .iter()
                .map(|(name, value)| (name.clone(), value.to_bits()))
                .collect(),
        }
    }

    #[cfg(any(test, target_arch = "wasm32"))]
    pub(super) fn clear(&mut self) {
        self.models.clear();
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn disk_filename(key: &PreparedSolveKey) -> String {
        use std::hash::{Hash, Hasher};

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        PREPARED_SOLVE_CACHE_VERSION.hash(&mut hasher);
        key.compiler_identity.hash(&mut hasher);
        key.source_key.hash(&mut hasher);
        key.solver_id.hash(&mut hasher);
        key.parameter_overrides.hash(&mut hasher);
        key.initial_inputs.hash(&mut hasher);
        let hash = hasher.finish();
        format!("{:016x}-{hash:016x}.bin.zst", key.source_key)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn disk_path(key: &PreparedSolveKey) -> std::path::PathBuf {
        modelica_dir()
            .join(format!("prepared-solve-v{PREPARED_SOLVE_CACHE_VERSION}"))
            .join(Self::disk_filename(key))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn load_disk(
        key: &PreparedSolveKey,
        limits: ModelicaArtifactDiskLimits,
    ) -> Result<Option<rumoca_ir_solve::fmi::FmiComponent>, ModelicaCacheReadError> {
        let path = Self::disk_path(key);
        Self::load_disk_at(&FileStorage::new(), &StorageHandle::File(path), key, limits)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn load_disk_at(
        storage: &FileStorage,
        handle: &StorageHandle,
        key: &PreparedSolveKey,
        limits: ModelicaArtifactDiskLimits,
    ) -> Result<Option<rumoca_ir_solve::fmi::FmiComponent>, ModelicaCacheReadError> {
        let Some(record) = read_record::<DecodedPreparedSolveDiskRecord>(storage, handle, limits)?
        else {
            return Ok(None);
        };
        if record.version != PREPARED_SOLVE_CACHE_VERSION
            || key.compiler_identity.is_none()
            || record.compiler_identity.as_deref() != key.compiler_identity
            || record.source_key != key.source_key
            || record.solver_id != key.solver_id
            || record.parameter_overrides != key.parameter_overrides
            || record.initial_inputs != key.initial_inputs
        {
            return Err(ModelicaCacheReadError::Identity);
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
        model: &rumoca_ir_solve::fmi::FmiComponent,
        limits: ModelicaArtifactDiskLimits,
    ) -> Result<(), ModelicaCacheWriteError> {
        let path = Self::disk_path(key);
        let root = path.parent().ok_or(ModelicaCacheWriteError::InvalidPath)?;
        Self::save_disk_at(&FileStorage::new(), root, &path, key, model, limits)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn encode_disk(
        key: &PreparedSolveKey,
        model: &rumoca_ir_solve::fmi::FmiComponent,
        limits: ModelicaArtifactDiskLimits,
    ) -> Result<Vec<u8>, ModelicaCacheWriteError> {
        limits
            .validate()
            .map_err(ModelicaCacheWriteError::InvalidLimits)?;
        let record = PreparedSolveDiskRecord {
            compiler_identity: Some(
                key.compiler_identity
                    .ok_or(ModelicaCacheWriteError::MissingCompilerIdentity)?
                    .to_string(),
            ),
            version: PREPARED_SOLVE_CACHE_VERSION,
            source_key: key.source_key,
            solver_id: key.solver_id.as_str(),
            parameter_overrides: key.parameter_overrides.as_slice(),
            initial_inputs: key.initial_inputs.as_slice(),
            model: rumoca_sim::runtime_fmi_component_wire(model)
                .map_err(|error| ModelicaCacheWriteError::ConstructionWire(error.to_string()))?,
        };
        encode_record(&record, limits)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn save_disk_at(
        storage: &FileStorage,
        root: &std::path::Path,
        path: &std::path::Path,
        key: &PreparedSolveKey,
        model: &rumoca_ir_solve::fmi::FmiComponent,
        limits: ModelicaArtifactDiskLimits,
    ) -> Result<(), ModelicaCacheWriteError> {
        if path.parent() != Some(root)
            || path.file_name().and_then(|name| name.to_str())
                != Some(Self::disk_filename(key).as_str())
        {
            return Err(ModelicaCacheWriteError::InvalidPath);
        }
        // Serialization/compression do not hold the cross-process publication lock.
        let compressed = Self::encode_disk(key, model, limits)?;
        let transaction = storage.lock_cache_directory(&StorageHandle::File(root.to_path_buf()))?;
        let incoming = transaction.directory().join(
            path.file_name()
                .ok_or(ModelicaCacheWriteError::InvalidPath)?,
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
        limits: ModelicaArtifactDiskLimits,
    ) -> Result<(), ModelicaCacheWriteError> {
        use std::collections::BTreeSet;
        // Keep the N-1 most recently published or used other valid-size
        // records; loads mark hits as used. Ties break by filename, so the
        // choice is independent of OS enumeration order.
        let mut recent = BTreeSet::new();
        for file in transaction.files()? {
            let file = file?;
            let StorageHandle::File(path) = file.handle else {
                return Err(ModelicaCacheWriteError::InvalidPath);
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
                    return Err(ModelicaCacheWriteError::InvalidPath);
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

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn read_record<T: serde::de::DeserializeOwned>(
    storage: &FileStorage,
    handle: &StorageHandle,
    limits: ModelicaArtifactDiskLimits,
) -> Result<Option<T>, ModelicaCacheReadError> {
    let Some(bytes) = read_payload(storage, handle, limits)? else {
        return Ok(None);
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(ModelicaCacheReadError::Codec)
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn read_payload(
    storage: &FileStorage,
    handle: &StorageHandle,
    limits: ModelicaArtifactDiskLimits,
) -> Result<Option<Vec<u8>>, ModelicaCacheReadError> {
    use std::io::Read;
    limits
        .validate()
        .map_err(ModelicaCacheReadError::InvalidLimits)?;
    let compressed =
        match bevy::tasks::block_on(storage.read_bounded(handle, limits.compressed_bytes)) {
            Ok(bytes) => bytes,
            Err(StorageError::NotFound) => return Ok(None),
            Err(error) => return Err(ModelicaCacheReadError::Storage(error)),
        };
    let mut decoder = zstd::stream::read::Decoder::with_buffer(compressed.as_slice())
        .map_err(ModelicaCacheReadError::Decompression)?;
    decoder
        .window_log_max(limits.zstd_window_log_max)
        .map_err(ModelicaCacheReadError::Decompression)?;
    let mut bytes = Vec::new();
    // One sentinel byte rejects an oversized stream without materializing it.
    decoder
        .take(limits.decoded_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(ModelicaCacheReadError::Decompression)?;
    if bytes.len() > limits.decoded_bytes {
        return Err(ModelicaCacheReadError::DecodedLimit {
            max_bytes: limits.decoded_bytes,
        });
    }
    Ok(Some(bytes))
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn encode_record<T: Serialize>(
    record: &T,
    limits: ModelicaArtifactDiskLimits,
) -> Result<Vec<u8>, ModelicaCacheWriteError> {
    encode_payload(limits, |writer| {
        serde_json::to_writer(writer, record).map_err(ModelicaCacheWriteError::Codec)
    })
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn encode_payload(
    limits: ModelicaArtifactDiskLimits,
    serialize: impl FnOnce(&mut dyn std::io::Write) -> Result<(), ModelicaCacheWriteError>,
) -> Result<Vec<u8>, ModelicaCacheWriteError> {
    limits
        .validate()
        .map_err(ModelicaCacheWriteError::InvalidLimits)?;
    let compressed = BudgetWriter::new(
        CacheBuffer::default(),
        limits.compressed_bytes,
        "compressed",
    );
    let mut encoder = zstd::stream::write::Encoder::new(compressed, 3)
        .map_err(ModelicaCacheWriteError::Compression)?;
    encoder
        .window_log(limits.zstd_window_log_max)
        .map_err(ModelicaCacheWriteError::Compression)?;
    encoder
        .include_checksum(true)
        .map_err(ModelicaCacheWriteError::Compression)?;
    // Batch serializer fragments before compression, with decoded admission
    // outside the buffer so oversized writes cannot enter it.
    let mut decoded = BudgetWriter::new(
        std::io::BufWriter::new(encoder),
        limits.decoded_bytes,
        "decoded",
    );
    serialize(&mut decoded)?;
    let compressed = decoded
        .inner
        .into_inner()
        .map_err(|error| ModelicaCacheWriteError::Compression(error.into_error()))?
        .finish()
        .map_err(ModelicaCacheWriteError::Compression)?;
    Ok(compressed.inner.0)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use lunco_storage::Storage;

    fn compiled_fixture() -> (
        super::super::ModelicaCompiler,
        super::super::CompileUnit,
        super::super::CompiledModelArtifact,
    ) {
        let unit = super::super::assemble_compile_unit(
            "model CompileCacheProbe parameter Real gain=0.10000000000000002; output Real y; equation y=gain; annotation(experiment(StartTime=0.25,StopTime=3.75,Tolerance=1e-9,Interval=0.125)); end CompileCacheProbe;",
            Vec::new(),
        );
        let mut compiler = super::super::ModelicaCompiler::new();
        let mut cache = CompiledArtifactCache::new(NonZeroUsize::new(2).unwrap());
        let artifact = super::super::compile_shared(
            &mut cache,
            &mut compiler,
            "CompileCacheProbe",
            &unit,
            "compile-cache.mo",
            0,
        )
        .unwrap();
        (compiler, unit, artifact)
    }

    #[test]
    fn compiled_artifact_round_trip_uses_native_checked_dae() {
        let (compiler, unit, artifact) = compiled_fixture();
        let limits = ModelicaArtifactDiskLimits::default();
        let mut cache = CompiledArtifactCache::new(NonZeroUsize::new(2).unwrap());
        cache.enable_disk(limits);
        let key = cache
            .disk_key(
                &compiler,
                "CompileCacheProbe",
                &unit,
                "compile-cache.mo",
                &[],
            )
            .unwrap()
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let handle = StorageHandle::File(directory.path().join("artifact.bin.zst"));
        let storage = FileStorage::new();
        storage
            .write_sync(
                &handle,
                &CompiledArtifactCache::encode_disk(&key, &artifact, limits).unwrap(),
            )
            .unwrap();
        let loaded = cache
            .load_disk_at(&storage, &handle, &key, &[], limits)
            .unwrap()
            .unwrap();
        let mut expected = serde_json::to_value(&artifact.compiled).unwrap();
        let mut actual = serde_json::to_value(&loaded.compiled).unwrap();
        expected.sort_all_objects();
        actual.sort_all_objects();
        assert_eq!(expected.to_string(), actual.to_string());
        assert_eq!(loaded.source_content, artifact.source_content);
        assert_eq!(loaded.prepared_source_key, artifact.prepared_source_key);
        assert_eq!(loaded.compiled.experiment_start_time, Some(0.25));
        assert_eq!(loaded.compiled.experiment_stop_time, Some(3.75));
        assert_eq!(loaded.compiled.experiment_tolerance, Some(1e-9));
        assert_eq!(loaded.compiled.experiment_interval, Some(0.125));

        let mut record: serde_json::Value =
            read_record(&storage, &handle, limits).unwrap().unwrap();
        record["compiled"]["dae"]["schema_version"] = serde_json::json!(0);
        storage
            .write_sync(&handle, &encode_record(&record, limits).unwrap())
            .unwrap();
        assert!(matches!(
            cache.load_disk_at(&storage, &handle, &key, &[], limits),
            Err(ModelicaCacheReadError::Codec(_))
        ));
    }

    #[test]
    fn compiled_artifact_identity_covers_inputs_and_exact_structural_bits() {
        let (mut compiler, unit, _) = compiled_fixture();
        let mut cache = CompiledArtifactCache::new(NonZeroUsize::new(2).unwrap());
        cache.enable_disk(ModelicaArtifactDiskLimits::default());
        let key = |compiler: &super::super::ModelicaCompiler,
                   unit: &super::super::CompileUnit,
                   uri,
                   numeric: &[(String, f64)]| {
            cache
                .disk_key(compiler, "CompileCacheProbe", unit, uri, numeric)
                .unwrap()
                .unwrap()
        };
        let first = key(&compiler, &unit, "compile-cache.mo", &[]);
        assert_eq!(
            first,
            key(
                &super::super::ModelicaCompiler::new(),
                &unit,
                "compile-cache.mo",
                &[]
            )
        );
        assert_ne!(first, key(&compiler, &unit, "another.mo", &[]));
        let extra = super::super::assemble_compile_unit(
            &unit.source,
            vec![(
                "extra.mo".into(),
                "model ExtraProbe Real x; equation x=1; end ExtraProbe;".into(),
            )],
        );
        assert_ne!(first, key(&compiler, &extra, "compile-cache.mo", &[]));
        let changed = super::super::assemble_compile_unit(
            &unit.source.replace("0.10000000000000002", "0.25"),
            Vec::new(),
        );
        assert_ne!(first, key(&compiler, &changed, "compile-cache.mo", &[]));
        assert_ne!(
            key(
                &compiler,
                &unit,
                "compile-cache.mo",
                &[("gain".into(), 0.0)]
            ),
            key(
                &compiler,
                &unit,
                "compile-cache.mo",
                &[("gain".into(), -0.0)]
            )
        );
        compiler.load_source_root_in_memory(
            "context-library",
            "ContextLibrary",
            vec![(
                "ContextLibrary/package.mo".into(),
                "package ContextLibrary end ContextLibrary;".into(),
            )],
        );
        assert_ne!(first, key(&compiler, &unit, "compile-cache.mo", &[]));
        let admitted = key(&compiler, &unit, "compile-cache.mo", &[]);
        compiler.load_source_root_in_memory(
            "context-library",
            "ContextLibrary",
            vec![(
                "ContextLibrary/package.mo".into(),
                "package ContextLibrary constant Real k=2; end ContextLibrary;".into(),
            )],
        );
        assert_ne!(admitted, key(&compiler, &unit, "compile-cache.mo", &[]));
        compiler.load_source_root_in_memory(
            "context-library",
            "ContextLibrary",
            vec![("ContextLibrary/package.mo".into(), "package ???".into())],
        );
        assert!(
            cache
                .disk_key(
                    &compiler,
                    "CompileCacheProbe",
                    &unit,
                    "compile-cache.mo",
                    &[]
                )
                .is_err()
        );
    }

    #[test]
    fn compiled_artifact_does_not_persist_uncaptured_external_resources() {
        let (_, _, mut artifact) = compiled_fixture();
        artifact.compiled.dae =
            serde_json::from_value(serde_json::to_value(&artifact.compiled.dae).unwrap()).unwrap();
        let dae = Arc::try_unwrap(artifact.compiled.dae).unwrap();
        artifact.compiled.dae =
            Arc::new(
                dae.with_external_tables(vec![rumoca_core::ExternalTableData {
                    id: 1,
                    data: vec![vec![0.0, 1.0], vec![1.0, 2.0]],
                    columns: vec![2],
                    smoothness: 1,
                    extrapolation: 1,
                }]),
            );
        let key = CompiledArtifactDiskKey {
            input: lunco_hash::content::cid(b"resource-dependent"),
        };
        assert!(matches!(
            CompiledArtifactCache::encode_disk(
                &key,
                &artifact,
                ModelicaArtifactDiskLimits::default()
            ),
            Err(ModelicaCacheWriteError::ConstructionWire(_))
        ));
    }

    fn key() -> PreparedSolveKey {
        PreparedSolveKey {
            compiler_identity: Some("test-compiler"),
            source_key: 11,
            solver_id: "generic-solver".into(),
            parameter_overrides: vec![("gain".into(), 3.5_f64.to_bits())],
            initial_inputs: Vec::new(),
        }
    }

    pub(super) fn component() -> rumoca_ir_solve::fmi::FmiComponent {
        let mut session = rumoca_compile::Session::new(rumoca_compile::SessionConfig::default());
        session.add_document("CacheProbe.mo", "model CacheProbe parameter Real gain = 3.5; Real x(start=42, fixed=true); equation der(x) = -gain*x; end CacheProbe;").unwrap();
        let compiled = session.compile_model("CacheProbe").unwrap();
        rumoca_sim::prepare_fmi_component(&compiled.dae, &rumoca_sim::SimOptions::default())
            .unwrap()
    }

    fn record(
        key: &PreparedSolveKey,
    ) -> PreparedSolveDiskRecord<rumoca_ir_solve::fmi::FmiComponent, String, Vec<(String, u64)>>
    {
        PreparedSolveDiskRecord {
            compiler_identity: key.compiler_identity.map(str::to_string),
            version: PREPARED_SOLVE_CACHE_VERSION,
            source_key: key.source_key,
            solver_id: key.solver_id.clone(),
            parameter_overrides: key.parameter_overrides.clone(),
            initial_inputs: key.initial_inputs.clone(),
            model: component(),
        }
    }

    fn encode(
        record: &PreparedSolveDiskRecord<
            rumoca_ir_solve::fmi::FmiComponent,
            String,
            Vec<(String, u64)>,
        >,
    ) -> Vec<u8> {
        let wire = PreparedSolveDiskRecord {
            compiler_identity: record.compiler_identity.clone(),
            version: record.version,
            source_key: record.source_key,
            solver_id: &record.solver_id,
            parameter_overrides: &record.parameter_overrides,
            initial_inputs: &record.initial_inputs,
            model: rumoca_sim::runtime_fmi_component_wire(&record.model).unwrap(),
        };
        serde_json::to_vec(&wire).unwrap()
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
        let limits = ModelicaArtifactDiskLimits {
            decoded_bytes: decoded_len,
            ..Default::default()
        };
        let compressed = PreparedSolveCache::encode_disk(&key, &record.model, limits).unwrap();
        let exact = ModelicaArtifactDiskLimits {
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
        assert_eq!(
            model.runtime_view().model().initial_y,
            record.model.runtime_view().model().initial_y
        );
        assert_eq!(
            model.runtime_view().model().parameters,
            record.model.runtime_view().model().parameters
        );
        let decoded_error = PreparedSolveCache::encode_disk(
            &key,
            &record.model,
            ModelicaArtifactDiskLimits {
                decoded_bytes: decoded_len - 1,
                ..exact
            },
        )
        .unwrap_err();
        assert!(decoded_error.to_string().contains("decoded byte budget"));
        let compressed_error = PreparedSolveCache::encode_disk(
            &key,
            &record.model,
            ModelicaArtifactDiskLimits {
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
        let limits = ModelicaArtifactDiskLimits {
            retained_entries: 2,
            compressed_bytes: 64 * 1024,
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
        assert_eq!(
            loaded.runtime_view().model().initial_y,
            model.runtime_view().model().initial_y
        );
    }

    #[test]
    fn persistent_solve_cache_concurrent_publications_enforce_exact_directory_quota() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("shared cache");
        let limits = ModelicaArtifactDiskLimits {
            retained_entries: 3,
            compressed_bytes: 64 * 1024,
            ..Default::default()
        };
        std::thread::scope(|scope| {
            for source in 0..8 {
                let directory = &directory;
                scope.spawn(move || {
                    let storage = FileStorage::new();
                    for parameter in 0..8 {
                        let mut key = key();
                        key.source_key = source;
                        key.parameter_overrides[0].1 = f64::from(parameter).to_bits();
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
                for parameter in 0..8 {
                    let mut key = key();
                    key.source_key = source;
                    key.parameter_overrides[0].1 = f64::from(parameter).to_bits();
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
                        assert_eq!(loaded.runtime_view().model().initial_y, [42.0]);
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
        let invalid = ModelicaArtifactDiskLimits {
            retained_entries: 0,
            ..Default::default()
        };
        assert!(matches!(
            PreparedSolveCache::save_disk_at(&storage, root.path(), &path, &key, &model, invalid),
            Err(ModelicaCacheWriteError::InvalidLimits(_))
        ));
        assert_eq!(ModelicaArtifactDiskLimits::default().retained_entries, 32);
        let (_command_tx, command_rx) = crossbeam_channel::unbounded();
        let (result_tx, result_rx) = crossbeam_channel::unbounded();
        super::super::modelica_worker(
            command_rx,
            result_tx,
            invalid,
            lunco_modelica_runtime::ModelicaCacheLimits::default(),
            super::super::ModelicaWorkerLimits::default(),
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
            Err(ModelicaCacheWriteError::InvalidPath)
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
            Err(ModelicaCacheWriteError::InvalidPath)
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
        assert!(matches!(error, ModelicaCacheWriteError::Storage(_)));
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
        let limits = ModelicaArtifactDiskLimits {
            compressed_bytes: compressed.len(),
            decoded_bytes: encoded.len(),
            ..Default::default()
        };
        let model = PreparedSolveCache::load_disk_at(&storage, &handle, &key, limits)
            .unwrap()
            .unwrap();
        assert_eq!(model.runtime_view().model().initial_y, [42.0]);
        assert_eq!(model.runtime_view().model().parameters, [3.5]);
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
                ModelicaArtifactDiskLimits {
                    compressed_bytes: compressed.len() - 1,
                    ..Default::default()
                },
            ),
            Err(ModelicaCacheReadError::Storage(
                StorageError::SizeLimitExceeded { .. }
            ))
        ));
        assert!(matches!(
            PreparedSolveCache::load_disk_at(
                &storage,
                &handle,
                &key,
                ModelicaArtifactDiskLimits {
                    decoded_bytes: encoded.len() - 1,
                    ..Default::default()
                },
            ),
            Err(ModelicaCacheReadError::DecodedLimit { .. })
        ));
        let (storage, handle) = stored(&compress(&vec![0; 1024 * 1024]));
        assert!(matches!(
            PreparedSolveCache::load_disk_at(
                &storage,
                &handle,
                &key,
                ModelicaArtifactDiskLimits {
                    decoded_bytes: 32,
                    ..Default::default()
                },
            ),
            Err(ModelicaCacheReadError::DecodedLimit { max_bytes: 32 })
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
                    ModelicaArtifactDiskLimits::default(),
                ),
                Err(ModelicaCacheReadError::Decompression(_))
            ));
        }
        let (storage, handle) = stored(&compress(&encoded[..encoded.len() - 1]));
        assert!(matches!(
            PreparedSolveCache::load_disk_at(
                &storage,
                &handle,
                &key,
                ModelicaArtifactDiskLimits::default(),
            ),
            Err(ModelicaCacheReadError::Codec(_))
        ));
        let mut trailing = encoded;
        trailing.push(0);
        let (storage, handle) = stored(&compress(&trailing));
        assert!(matches!(
            PreparedSolveCache::load_disk_at(
                &storage,
                &handle,
                &key,
                ModelicaArtifactDiskLimits::default(),
            ),
            Err(ModelicaCacheReadError::Codec(_))
        ));
    }

    #[test]
    fn prepared_solve_disk_cache_rejects_every_identity_mismatch() {
        let key = key();
        for field in 0..7 {
            let mut record = record(&key);
            match field {
                0 => record.version += 1,
                1 => record.source_key += 1,
                2 => record.solver_id.push('x'),
                3 => record.parameter_overrides[0].1 = 7.0_f64.to_bits(),
                4 => record
                    .initial_inputs
                    .push(("forcing".into(), 7.0_f64.to_bits())),
                5 => record.compiler_identity = Some("another-compiler".into()),
                6 => record.compiler_identity = None,
                _ => unreachable!(),
            }
            let (storage, handle) = stored(&compress(&encode(&record)));
            assert!(
                matches!(
                    PreparedSolveCache::load_disk_at(
                        &storage,
                        &handle,
                        &key,
                        ModelicaArtifactDiskLimits::default(),
                    ),
                    Err(ModelicaCacheReadError::Identity)
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
                ModelicaArtifactDiskLimits {
                    zstd_window_log_max: 10,
                    ..Default::default()
                },
            ),
            Err(ModelicaCacheReadError::Decompression(_))
        ));
    }

    #[test]
    fn prepared_solve_disk_cache_rejects_invalid_limits_before_worker_start() {
        let invalid = ModelicaArtifactDiskLimits {
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
            super::super::ModelicaWorkerLimits::default(),
        );
        let result = result_rx.try_recv().unwrap();
        assert!(
            result
                .worker_failure
                .as_deref()
                .unwrap()
                .contains("invalid Modelica artifact disk-cache limits")
        );
        for limits in [
            ModelicaArtifactDiskLimits {
                compressed_bytes: 0,
                ..Default::default()
            },
            ModelicaArtifactDiskLimits {
                decoded_bytes: ModelicaArtifactDiskLimits::CODEC_MAX_DECODED_BYTES + 1,
                ..Default::default()
            },
            ModelicaArtifactDiskLimits {
                zstd_window_log_max: 9,
                ..Default::default()
            },
            ModelicaArtifactDiskLimits {
                zstd_window_log_max: ModelicaArtifactDiskLimits::MAX_WINDOW_LOG + 1,
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
    fn immutable_reuse_cache_bounds_entries_and_refreshes_hits_and_replacement() {
        let mut cache = BoundedReuseCache::new(NonZeroUsize::new(2).unwrap());
        cache.insert(1, "first");
        cache.insert(2, "second");
        assert_eq!(cache.get(&1), Some(&"first"));
        assert_eq!(cache.len(), 2);
        cache.insert(3, "third");
        assert!(!cache.contains_key(&2));
        assert_eq!(cache.get(&3), Some(&"third"));
        cache.insert(1, "replacement");
        cache.insert(4, "fourth");
        assert!(!cache.contains_key(&3));
        assert_eq!(cache.get(&1), Some(&"replacement"));
        for key in 4..1000 {
            cache.insert(key, "new graph");
        }
        assert_eq!(cache.len(), 2);
        assert!(cache.contains_key(&998));
        assert!(cache.contains_key(&999));
        cache.clear();
        assert_eq!(cache.len(), 0);
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
            compiler_identity: Some("test-compiler"),
            source_key: 1,
            solver_id: "generic-solver".into(),
            parameter_overrides: vec![("gain".into(), 3.0_f64.to_bits())],
            initial_inputs: Vec::new(),
        };
        let graph = Arc::new(super::tests::component());
        cache.insert(key.clone(), Arc::clone(&graph));
        assert!(Arc::ptr_eq(cache.get(&key).unwrap(), &graph));
        let mut parameter_key = key.clone();
        parameter_key.parameter_overrides[0].1 = 4.0_f64.to_bits();
        assert!(!cache.contains_key(&parameter_key));
        let mut input_key = key.clone();
        input_key
            .initial_inputs
            .push(("drive".into(), 2.0_f64.to_bits()));
        assert!(!cache.contains_key(&input_key));
        cache.insert(parameter_key.clone(), Arc::clone(&graph));
        let mut solver_key = key.clone();
        solver_key.solver_id.push('x');
        cache.insert(solver_key.clone(), Arc::clone(&graph));
        assert!(!cache.contains_key(&key));
        assert!(cache.contains_key(&parameter_key));
        assert!(cache.contains_key(&solver_key));
        cache.clear();
        assert!(!cache.contains_key(&solver_key));
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
            ModelicaCacheLimits {
                native_code_entries: 0,
                ..Default::default()
            },
            ModelicaCacheLimits {
                native_code_bytes: 0,
                ..Default::default()
            },
        ] {
            assert!(limits.validate().is_err());
        }
        let capacities = ModelicaCacheLimits::default().validate().unwrap();
        assert_eq!(capacities.compiled_artifacts.get(), 64);
        assert_eq!(capacities.prepared_solves.get(), 64);
        assert_eq!(capacities.native_code_entries.get(), 32_768);
        assert_eq!(capacities.native_code_bytes.get(), 64 * 1024 * 1024);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn immutable_reuse_cache_limits_fail_worker_before_thread_admission() {
        let (_command_tx, command_rx) = crossbeam_channel::unbounded();
        let (result_tx, result_rx) = crossbeam_channel::unbounded();
        super::super::modelica_worker(
            command_rx,
            result_tx,
            ModelicaArtifactDiskLimits::default(),
            ModelicaCacheLimits {
                compiled_artifact_entries: 0,
                ..Default::default()
            },
            super::super::ModelicaWorkerLimits::default(),
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
