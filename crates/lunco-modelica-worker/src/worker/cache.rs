//! In-memory and persistent compiled-DAE cache.

use linked_hash_map::LinkedHashMap;
#[cfg(not(target_arch = "wasm32"))]
use lunco_assets_core::modelica_dir;
#[cfg(not(target_arch = "wasm32"))]
use lunco_storage::{FileStorage, Storage, StorageError, StorageHandle};
#[cfg(not(target_arch = "wasm32"))]
use serde::{Deserialize, Serialize};
use std::hash::Hash;
use std::num::NonZeroUsize;
#[cfg(test)]
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

    pub(super) fn compatible_variant(
        &mut self,
        binding_schema: &lunco_hash::content::Cid,
        values: &[(String, f64)],
    ) -> Option<(u64, super::CompiledModelArtifact)> {
        let key = self.entries.ordered_entries().find_map(|(key, artifact)| {
            (artifact.binding_schema == *binding_schema
                && artifact.structural_overrides.iter().all(|(name, value)| {
                    values.iter().any(|(requested_name, requested)| {
                        requested_name == name && requested.to_bits() == value.to_bits()
                    })
                }))
            .then_some(*key)
        })?;
        self.entries
            .get(&key)
            .cloned()
            .map(|artifact| (key, artifact))
    }

    pub(super) fn checked_request(
        &mut self,
        identity: &lunco_hash::content::Cid,
    ) -> Option<super::CompiledModelArtifact> {
        let key = self.entries.ordered_entries().find_map(|(key, artifact)| {
            (artifact.checked_request.as_ref() == Some(identity)).then_some(*key)
        })?;
        self.entries.get(&key).cloned()
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
    const DISK_VERSION: u32 = 6;

    pub(super) fn enable_disk(&mut self, limits: ModelicaArtifactDiskLimits) {
        self.disk_limits = Some(limits);
    }

    pub(super) fn disk_key(
        &self,
        source_identity: &lunco_hash::content::Cid,
        numeric: &[(String, f64)],
    ) -> Option<CompiledArtifactDiskKey> {
        self.disk_limits?;
        let identity = rumoca_tool_build_info::build_identity()
            .filter(|identity| !identity.ends_with("-dirty"))?;
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
        field(&source_identity.to_bytes());
        field(&(numeric.len() as u64).to_le_bytes());
        for (name, value) in numeric {
            field(name.as_bytes());
            field(&value.to_bits().to_le_bytes());
        }
        Some(CompiledArtifactDiskKey {
            input: hash.finish(),
        })
    }

    fn disk_path(key: &CompiledArtifactDiskKey) -> std::path::PathBuf {
        let bytes = key.input.to_bytes();
        let first = lunco_hash::fnv1a64(&bytes);
        let second = lunco_hash::fnv1a64(key.input.to_string().as_bytes());
        modelica_dir()
            .join(format!("compiled-dae-v{}", Self::DISK_VERSION))
            .join(format!("{first:016x}-{second:016x}.bin.zst"))
    }

    pub(super) fn load_disk(
        &self,
        key: &CompiledArtifactDiskKey,
        numeric: &[(String, f64)],
        binding_schema: lunco_hash::content::Cid,
    ) -> Result<Option<super::CompiledModelArtifact>, ModelicaCacheReadError> {
        let Some(limits) = self.disk_limits else {
            return Ok(None);
        };
        self.load_disk_at(
            &FileStorage::new(),
            &StorageHandle::File(Self::disk_path(key)),
            key,
            numeric,
            binding_schema,
            limits,
        )
    }

    fn load_disk_at(
        &self,
        storage: &FileStorage,
        handle: &StorageHandle,
        key: &CompiledArtifactDiskKey,
        numeric: &[(String, f64)],
        binding_schema: lunco_hash::content::Cid,
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
        if record.version != Self::DISK_VERSION || record.input != key.input.to_string() {
            return Err(ModelicaCacheReadError::Identity);
        }
        let checked_structural =
            super::ModelicaCompiler::structural_parameter_overrides(&record.compiled.dae, numeric)
                .map_err(|_| ModelicaCacheReadError::Identity)?;
        let expected_structural = numeric
            .iter()
            .filter(|(name, _)| {
                checked_structural
                    .iter()
                    .any(|(structural, _)| structural == name)
            })
            .map(|(name, value)| (name.clone(), value.to_bits()))
            .collect::<Vec<_>>();
        if record.structural_overrides != expected_structural {
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
            structural_overrides: numeric
                .iter()
                .filter(|(name, _)| {
                    checked_structural
                        .iter()
                        .any(|(structural, _)| structural == name)
                })
                .cloned()
                .collect(),
            authored_bindings: Default::default(),
            edit_interface: None,
            checked_request: None,
            source_content,
            binding_schema,
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
        retain_disk_artifacts(&storage, &transaction, &incoming, compressed.len(), limits)?;
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

    #[cfg(test)]
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
    /// Maximum compressed bytes across one artifact family after publication.
    pub retained_bytes: u64,
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
            || self.retained_bytes < self.compressed_bytes as u64
            || self.decoded_bytes == 0
            || self.decoded_bytes > Self::CODEC_MAX_DECODED_BYTES
            || !(10..=Self::MAX_WINDOW_LOG).contains(&self.zstd_window_log_max)
        {
            return Err(format!(
                "invalid Modelica artifact disk-cache limits: retained entries must be positive, the directory byte budget must fit one maximum compressed record, compressed/decoded byte budgets must be 1..={} and zstd window log must be 10..={}",
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
            retained_entries: 128,
            retained_bytes: 256 * 1024 * 1024,
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
    InvalidPath,
    ConstructionWire(String),
    Codec(serde_json::Error),
    Compression(std::io::Error),
    Storage(StorageError),
}

#[cfg(not(target_arch = "wasm32"))]
impl std::fmt::Display for ModelicaCacheWriteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLimits(error) => formatter.write_str(error),
            Self::ConstructionWire(error) => write!(
                formatter,
                "checked component serialization rejected: {error}"
            ),
            Self::InvalidPath => {
                formatter.write_str("publication is outside its owned cache namespace")
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

#[cfg(not(target_arch = "wasm32"))]
fn retain_disk_artifacts(
    storage: &FileStorage,
    transaction: &lunco_storage::file_storage::DirectoryCacheTransaction,
    incoming: &std::path::Path,
    incoming_bytes: usize,
    limits: ModelicaArtifactDiskLimits,
) -> Result<(), ModelicaCacheWriteError> {
    use std::collections::{BTreeMap, BTreeSet};
    let Some(mut remaining_bytes) = limits.retained_bytes.checked_sub(incoming_bytes as u64) else {
        return Err(ModelicaCacheWriteError::InvalidLimits(
            "compressed artifact exceeds the directory byte budget".into(),
        ));
    };
    // Loads record use in the modification time. Select a bounded prefix by
    // recency, with filenames breaking ties independently of directory order.
    let mut recent = BTreeMap::new();
    for file in transaction.files()? {
        let file = file?;
        let StorageHandle::File(path) = file.handle else {
            return Err(ModelicaCacheWriteError::InvalidPath);
        };
        if path != incoming
            && owned_cache_filename(&path)
            && file.bytes <= limits.compressed_bytes as u64
        {
            recent.insert((std::cmp::Reverse(file.modified), path), file.bytes);
            if recent.len() >= limits.retained_entries {
                recent.pop_last();
            }
        }
    }
    let mut keep = BTreeSet::new();
    for ((_, path), bytes) in recent {
        let Some(remaining) = remaining_bytes.checked_sub(bytes) else {
            break;
        };
        remaining_bytes = remaining;
        keep.insert(path);
    }
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
        let source_identity = compiler
            .source_unit_identity(
                "CompileCacheProbe",
                &unit.source,
                "compile-cache.mo",
                &unit.extras,
            )
            .unwrap();
        let key = cache.disk_key(&source_identity, &[]).unwrap();
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
            .load_disk_at(
                &storage,
                &handle,
                &key,
                &[],
                artifact.binding_schema,
                limits,
            )
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
            cache.load_disk_at(
                &storage,
                &handle,
                &key,
                &[],
                artifact.binding_schema,
                limits
            ),
            Err(ModelicaCacheReadError::Codec(_))
        ));
    }

    #[test]
    fn compiled_artifact_identity_covers_source_unit_and_structural_bits() {
        let (mut compiler, unit, _) = compiled_fixture();
        let mut cache = CompiledArtifactCache::new(NonZeroUsize::new(2).unwrap());
        cache.enable_disk(ModelicaArtifactDiskLimits::default());
        let key = |compiler: &mut super::super::ModelicaCompiler,
                   unit: &super::super::CompileUnit,
                   uri,
                   numeric: &[(String, f64)]| {
            let identity = compiler
                .source_unit_identity("CompileCacheProbe", &unit.source, uri, &unit.extras)
                .unwrap();
            cache.disk_key(&identity, numeric).unwrap()
        };
        let first = key(&mut compiler, &unit, "compile-cache.mo", &[]);
        assert_eq!(
            first,
            key(
                &mut super::super::ModelicaCompiler::new(),
                &unit,
                "compile-cache.mo",
                &[]
            )
        );
        assert_ne!(first, key(&mut compiler, &unit, "another.mo", &[]));
        let extra = super::super::assemble_compile_unit(
            &unit.source,
            vec![(
                "extra.mo".into(),
                "model ExtraProbe Real x=1; end ExtraProbe;".into(),
            )],
        );
        assert_ne!(first, key(&mut compiler, &extra, "compile-cache.mo", &[]));
        let changed = super::super::assemble_compile_unit(
            &unit.source.replace("0.10000000000000002", "0.25"),
            Vec::new(),
        );
        assert_ne!(first, key(&mut compiler, &changed, "compile-cache.mo", &[]));
        assert_ne!(
            key(
                &mut compiler,
                &unit,
                "compile-cache.mo",
                &[("gain".into(), 0.0)]
            ),
            key(
                &mut compiler,
                &unit,
                "compile-cache.mo",
                &[("gain".into(), -0.0)]
            ),
        );

        let dependency = super::super::assemble_compile_unit(
            "model CompileCacheProbe Library.Value part; end CompileCacheProbe;",
            Vec::new(),
        );
        let install = |compiler: &mut super::super::ModelicaCompiler, used: u32, unused: u32| {
            let report = compiler.load_source_root_in_memory(
                "library",
                "Library",
                vec![
                    (
                        "Library/package.mo".into(),
                        "package Library end Library;".into(),
                    ),
                    (
                        "Library/Value.mo".into(),
                        format!("within Library; model Value Real x={used}; end Value;"),
                    ),
                    (
                        "Library/Unused.mo".into(),
                        format!("within Library; model Unused Real x={unused}; end Unused;"),
                    ),
                ],
            );
            assert!(report.diagnostics.is_empty(), "{report:?}");
        };
        install(&mut compiler, 1, 1);
        let first = key(&mut compiler, &dependency, "compile-cache.mo", &[]);
        install(&mut compiler, 1, 2);
        assert_ne!(
            first,
            key(&mut compiler, &dependency, "compile-cache.mo", &[])
        );
        install(&mut compiler, 2, 2);
        assert_ne!(
            first,
            key(&mut compiler, &dependency, "compile-cache.mo", &[])
        );
        compiler.remove_source_root("library");
        assert!(
            compiler
                .compile_str_multi_after_source_root_admission(
                    "CompileCacheProbe",
                    &dependency.source,
                    "compile-cache.mo",
                    &[],
                    &[],
                )
                .is_err()
        );

        let scoped = super::super::assemble_compile_unit(
            "within Scope; model CompileCacheProbe Item part; end CompileCacheProbe;",
            Vec::new(),
        );
        let scope = ("Scope/package.mo".into(), "package Scope end Scope;".into());
        let global = ("Item.mo".into(), "model Item Real x=1; end Item;".into());
        assert!(
            compiler
                .load_source_root_in_memory("scope", "Scope", vec![scope.clone(), global.clone()])
                .diagnostics
                .is_empty()
        );
        let unshadowed = key(&mut compiler, &scoped, "compile-cache.mo", &[]);
        compiler.clear_user_documents();
        let local = (
            "Scope/Item.mo".into(),
            "within Scope; model Item Real x=2; end Item;".into(),
        );
        assert!(
            compiler
                .load_source_root_in_memory("scope", "Scope", vec![scope, global, local])
                .diagnostics
                .is_empty()
        );
        assert_ne!(
            unshadowed,
            key(&mut compiler, &scoped, "compile-cache.mo", &[])
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
    fn immutable_reuse_cache_limits_reject_zero_capacity() {
        let limits = ModelicaCacheLimits {
            compiled_artifact_entries: 0,
            ..Default::default()
        };
        assert!(limits.validate().is_err());
        let capacities = ModelicaCacheLimits::default().validate().unwrap();
        assert_eq!(capacities.compiled_artifacts.get(), 64);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn immutable_reuse_cache_limits_fail_worker_before_thread_admission() {
        let (_command_tx, command_rx) = crossbeam_channel::unbounded();
        let (result_tx, result_rx) = crossbeam_channel::unbounded();
        super::super::modelica_worker(
            command_rx,
            lunco_modelica_runtime::ModelicaResponseSender::new(
                result_tx,
                crossbeam_channel::unbounded().0,
            ),
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
