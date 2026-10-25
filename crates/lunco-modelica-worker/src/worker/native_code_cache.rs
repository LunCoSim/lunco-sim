//! Application-lived, bounded native compiler artifacts; no executable or FMI state.

use super::cache::{BoundedReuseCache, ModelicaArtifactDiskLimits, encode_record, read_record};
use super::thread_pool::Lane;
use lunco_modelica_runtime::{ModelicaCacheCapacities, ModelicaNativeCodeCacheProfile};
use lunco_storage::{FileStorage, Storage, StorageHandle};
use rumoca_sim::native_code_cache::{CacheKvStore, with_native_code_cache};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard};

type Key = [u8; 32];
const NATIVE_CODE_CACHE_VERSION: u32 = 2;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    entries: Vec<Entry>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    key: Key,
    code: Arc<[u8]>,
}

impl Record {
    fn validate(&self) -> Result<(), String> {
        if self.version != NATIVE_CODE_CACHE_VERSION {
            return Err(format!(
                "unsupported native artifact record version {}",
                self.version
            ));
        }
        let mut keys = std::collections::HashSet::new();
        for entry in &self.entries {
            if entry.code.is_empty() || !keys.insert(entry.key) {
                return Err("native artifact record has an empty value or duplicate key".to_owned());
            }
        }
        Ok(())
    }
}

struct CachedCode {
    code: Arc<[u8]>,
    used_at: Cell<u64>,
}

struct Data {
    entries: BoundedReuseCache<Key, CachedCode>,
    bytes: usize,
    max_bytes: usize,
    /// Local access order; publication never uses wall time.
    revision: u64,
    /// Last insertion requiring persistence, distinct from read-only touches.
    modified: u64,
    /// Access watermark advanced only after a successful disk merge.
    published: u64,
    requested: u64,
    attempted: u64,
    writer_active: bool,
}

impl Data {
    fn new(capacities: ModelicaCacheCapacities) -> Self {
        Self {
            entries: BoundedReuseCache::new(capacities.native_code_entries),
            bytes: 0,
            max_bytes: capacities.native_code_bytes.get(),
            revision: 0,
            modified: 0,
            published: 0,
            requested: 0,
            attempted: 0,
            writer_active: false,
        }
    }

    fn get(&mut self, key: &Key) -> Option<Arc<[u8]>> {
        let entry = self.entries.get(key)?;
        self.revision += 1;
        entry.used_at.set(self.revision);
        Some(Arc::clone(&entry.code))
    }

    fn admit(&mut self, key: Key, code: Arc<[u8]>, used_at: u64) -> Option<u64> {
        let weight = key.len() + code.len();
        if code.is_empty() || weight > self.max_bytes {
            return None;
        }
        let mut evictions = 0;
        loop {
            let replaced = self
                .entries
                .get(&key)
                .map_or(0, |old| key.len() + old.code.len());
            if self.bytes - replaced + weight <= self.max_bytes {
                break;
            }
            if let Some(old) = self.entries.pop_oldest() {
                self.bytes -= key.len() + old.code.len();
                evictions += 1;
            }
        }
        let replacing = self.entries.contains_key(&key);
        if let Some(old) = self.entries.insert(
            key,
            CachedCode {
                code,
                used_at: Cell::new(used_at),
            },
        ) {
            self.bytes -= key.len() + old.code.len();
            evictions += u64::from(!replacing);
        }
        self.bytes += weight;
        Some(evictions)
    }

    fn insert(&mut self, key: Key, code: Arc<[u8]>) -> u64 {
        let weight = key.len() + code.len();
        let revision = self.revision + 1;
        let Some(evictions) = self.admit(key, code, revision) else {
            return 0;
        };
        self.revision = revision;
        self.modified = revision;
        bevy::log::trace!(
            operation = "insert",
            key = ?key,
            code_bytes = weight - key.len(),
            evictions,
            retained_entries = self.entries.len(),
            retained_bytes = self.bytes,
            "native-code-cache-operation"
        );
        evictions
    }

    fn snapshot(&self, after: Option<u64>) -> Vec<(Key, Arc<[u8]>)> {
        self.entries
            .ordered_entries()
            .filter(|(_, entry)| after.is_none_or(|revision| entry.used_at.get() > revision))
            .map(|(key, entry)| (*key, Arc::clone(&entry.code)))
            .collect()
    }

    fn import_older(&mut self, record: Record, capacities: ModelicaCacheCapacities) {
        let mut merged = Self::new(capacities);
        for entry in record.entries {
            let _ = merged.admit(entry.key, entry.code, 0);
        }
        for (key, entry) in self.entries.ordered_entries() {
            let _ = merged.admit(*key, Arc::clone(&entry.code), entry.used_at.get());
        }
        // A late load keeps current values, order and unpublished access stamps.
        self.entries = merged.entries;
        self.bytes = merged.bytes;
    }
}

#[derive(Clone)]
pub(super) struct SharedNativeCodeCache {
    data: Arc<Mutex<Data>>,
    io: Lane<()>,
    limits: ModelicaArtifactDiskLimits,
    capacities: ModelicaCacheCapacities,
    path: StorageHandle,
}

impl SharedNativeCodeCache {
    pub(super) fn new(
        capacities: ModelicaCacheCapacities,
        limits: ModelicaArtifactDiskLimits,
        io: Lane<()>,
    ) -> Result<Self, String> {
        let cache = Self {
            data: Arc::new(Mutex::new(Data::new(capacities))),
            io,
            limits,
            capacities,
            path: StorageHandle::File(
                lunco_assets_core::modelica_dir()
                    .join(format!("native-code-v{NATIVE_CODE_CACHE_VERSION}"))
                    .join("stencils.json.zst"),
            ),
        };
        bevy::log::debug!(
            path = ?cache.path,
            capacity_entries = capacities.native_code_entries.get(),
            capacity_bytes = capacities.native_code_bytes.get(),
            "native code cache admission"
        );
        let loading = cache.clone();
        cache.io.submit(move |_| {
            if let Err(error) = loading.load() {
                bevy::log::warn!("[modelica-runtime] native code cache load rejected: {error}");
            }
        })?;
        Ok(cache)
    }

    fn lock(&self) -> Result<MutexGuard<'_, Data>, String> {
        self.data
            .lock()
            .map_err(|_| "native code cache owner is poisoned".to_owned())
    }

    fn load(&self) -> Result<(), String> {
        let _span = bevy::log::info_span!("modelica_native_cache_load").entered();
        let Some(record) = read_record::<Record>(&FileStorage::new(), &self.path, self.limits)
            .map_err(|error| error.to_string())?
        else {
            bevy::log::info!("[modelica-runtime] native code artifact cache is cold");
            return Ok(());
        };
        record.validate()?;
        let loaded_entries = record.entries.len();
        let mut data = self.lock()?;
        data.import_older(record, self.capacities);
        bevy::log::info!(
            loaded_entries,
            retained_entries = data.entries.len(),
            retained_bytes = data.bytes,
            "[modelica-runtime] native code artifacts loaded"
        );
        Ok(())
    }

    fn request_publication(&self) -> Result<(), String> {
        {
            let mut data = self.lock()?;
            data.requested = data.requested.max(data.modified);
            if data.writer_active || data.requested <= data.attempted {
                return Ok(());
            }
            data.writer_active = true;
        }
        let cache = self.clone();
        if let Err(error) = self.io.submit(move |_| cache.publish()) {
            self.lock()?.writer_active = false;
            return Err(error);
        }
        Ok(())
    }

    fn publish(&self) {
        loop {
            let snapshot = self.lock().map(|mut data| {
                if data.requested == data.attempted {
                    data.writer_active = false;
                    return None;
                }
                let revision = data.revision;
                Some((revision, data.snapshot(Some(data.published))))
            });
            let (revision, entries) = match snapshot {
                Ok(Some(snapshot)) => snapshot,
                Ok(None) => return,
                Err(error) => {
                    bevy::log::warn!(
                        "[modelica-runtime] native code cache snapshot failed: {error}"
                    );
                    return;
                }
            };
            let result = self.publish_delta(entries);
            let published = result.is_ok();
            if let Err(error) = result {
                bevy::log::warn!(
                    "[modelica-runtime] native code cache publication failed: {error}"
                );
            }
            match self.lock() {
                Ok(mut data) => {
                    data.attempted = revision;
                    if published {
                        data.published = revision;
                    }
                    if data.requested <= revision {
                        data.requested = revision;
                    }
                }
                Err(error) => {
                    bevy::log::warn!(
                        "[modelica-runtime] native code cache completion failed: {error}"
                    );
                    return;
                }
            }
        }
    }

    fn publish_delta(&self, entries: Vec<(Key, Arc<[u8]>)>) -> Result<(), String> {
        let _span = bevy::log::info_span!("modelica_native_cache_publish").entered();
        let StorageHandle::File(path) = &self.path else {
            return Err("native artifacts require a file cache".to_owned());
        };
        let root = path.parent().ok_or("native cache path has no parent")?;
        let name = path
            .file_name()
            .ok_or("native cache path has no filename")?;
        let storage = FileStorage::new();
        let transaction = storage
            .lock_cache_directory(&StorageHandle::File(root.to_path_buf()))
            .map_err(|error| error.to_string())?;
        // The storage guard covers the complete read/merge/atomic-replace operation.
        let path = StorageHandle::File(transaction.directory().join(name));
        let mut merged = Data::new(self.capacities);
        if let Some(record) = read_record::<Record>(&storage, &path, self.limits)
            .map_err(|error| error.to_string())?
        {
            record.validate()?;
            merged.import_older(record, self.capacities);
        }
        for (key, code) in entries {
            let _ = merged.admit(key, code, 0);
        }
        let record = Record {
            version: NATIVE_CODE_CACHE_VERSION,
            entries: merged
                .snapshot(None)
                .into_iter()
                .map(|(key, code)| Entry { key, code })
                .collect(),
        };
        let bytes = encode_record(&record, self.limits).map_err(|error| error.to_string())?;
        storage
            .write_sync(&path, &bytes)
            .map_err(|error| error.to_string())
    }

    pub(super) fn construct<R>(
        &self,
        compile: impl FnOnce() -> Result<R, rumoca_sim::SimulationDiagnosticError>,
    ) -> Result<(R, ModelicaNativeCodeCacheProfile), rumoca_sim::SimulationDiagnosticError> {
        let store = Rc::new(RefCell::new(Adapter {
            cache: self.clone(),
            fault: RefCell::new(None),
            evictions: Cell::new(0),
        }));
        let (result, usage) = with_native_code_cache(store.clone(), compile);
        if let Some(error) = store.borrow().fault.borrow_mut().take() {
            return Err(rumoca_sim::SimulationDiagnosticError::Solver(error));
        }
        let result = result?;
        if usage.rejected_entries != 0 {
            bevy::log::warn!(
                "[modelica-runtime] rejected {} native code artifacts; compiled fresh code",
                usage.rejected_entries
            );
        }
        if let Err(error) = self.request_publication() {
            bevy::log::warn!(
                "[modelica-runtime] native code cache publication unavailable: {error}"
            );
        }
        let (retained_entries, retained_bytes) = {
            let data = self
                .lock()
                .map_err(rumoca_sim::SimulationDiagnosticError::Solver)?;
            (data.entries.len() as u64, data.bytes as u64)
        };
        Ok((
            result,
            ModelicaNativeCodeCacheProfile {
                attempts: usage.attempts,
                hits: usage.hits,
                compilations: usage.compilations,
                rejected_entries: usage.rejected_entries,
                evictions: store.borrow().evictions.get(),
                retained_entries,
                retained_bytes,
            },
        ))
    }
}

struct Adapter {
    cache: SharedNativeCodeCache,
    fault: RefCell<Option<String>>,
    evictions: Cell<u64>,
}

impl Adapter {
    fn key(&self, key: &[u8]) -> Option<Key> {
        match key.try_into() {
            Ok(key) => Some(key),
            Err(_) => {
                *self.fault.borrow_mut() = Some(format!(
                    "native compiler emitted a {}-byte cache key; expected 32",
                    key.len()
                ));
                None
            }
        }
    }
}

impl CacheKvStore for Adapter {
    fn get(&self, key: &[u8]) -> Option<Cow<'_, [u8]>> {
        let key = self.key(key)?;
        match self.cache.lock() {
            Ok(mut data) => {
                let code: Option<Cow<'_, [u8]>> =
                    data.get(&key).map(|code| Cow::Owned(code.to_vec()));
                bevy::log::trace!(
                    operation = "get",
                    key = ?key,
                    hit = code.is_some(),
                    code_bytes = code.as_ref().map_or(0, |code| code.len()),
                    "native-code-cache-operation"
                );
                code
            }
            Err(error) => {
                *self.fault.borrow_mut() = Some(error);
                None
            }
        }
    }

    fn insert(&mut self, key: &[u8], code: Vec<u8>) {
        let Some(key) = self.key(key) else {
            return;
        };
        match self.cache.lock() {
            Ok(mut data) => self
                .evictions
                .set(self.evictions.get() + data.insert(key, code.into())),
            Err(error) => *self.fault.borrow_mut() = Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capacities(entries: usize, bytes: usize) -> ModelicaCacheCapacities {
        lunco_modelica_runtime::ModelicaCacheLimits {
            native_code_entries: entries,
            native_code_bytes: bytes,
            ..Default::default()
        }
        .validate()
        .unwrap()
    }

    fn cache_at(path: &StorageHandle, io: Lane<()>) -> SharedNativeCodeCache {
        let capacities = capacities(3, 1000);
        SharedNativeCodeCache {
            data: Arc::new(Mutex::new(Data::new(capacities))),
            io,
            limits: ModelicaArtifactDiskLimits::default(),
            capacities,
            path: path.clone(),
        }
    }

    fn publish_now(cache: &SharedNativeCodeCache) {
        let mut data = cache.lock().unwrap();
        data.requested = data.revision;
        drop(data);
        cache.publish();
    }

    #[test]
    fn native_code_cache_large_record_roundtrip() {
        let record = Record {
            version: NATIVE_CODE_CACHE_VERSION,
            entries: (0u32..4096)
                .map(|index| {
                    let mut key = [0; 32];
                    key[..4].copy_from_slice(&index.to_le_bytes());
                    Entry {
                        key,
                        code: (0u32..512)
                            .map(|offset| index.wrapping_mul(31).wrapping_add(offset) as u8)
                            .collect::<Vec<_>>()
                            .into(),
                    }
                })
                .collect(),
        };
        let limits = ModelicaArtifactDiskLimits::default();
        let started = std::time::Instant::now();
        let encoded = encode_record(&record, limits).unwrap();
        let encoding = started.elapsed();
        let dir = tempfile::tempdir().unwrap();
        let handle = StorageHandle::File(dir.path().join("native.zst"));
        let storage = FileStorage::new();
        storage.write_sync(&handle, &encoded).unwrap();
        let started = std::time::Instant::now();
        let loaded = read_record::<Record>(&storage, &handle, limits)
            .unwrap()
            .unwrap();
        let decoding = started.elapsed();
        loaded.validate().unwrap();
        assert_eq!(loaded.entries.len(), record.entries.len());
        for (actual, expected) in loaded.entries.iter().zip(&record.entries) {
            assert_eq!(actual.key, expected.key);
            assert_eq!(actual.code, expected.code);
        }
        println!(
            "native cache 4096-entry codec diagnostic: encode={encoding:?}, decode={decoding:?}, compressed_bytes={}",
            encoded.len()
        );
    }

    #[test]
    fn native_code_cache_preserves_other_writers_and_recent_reads() {
        let directory = tempfile::tempdir().unwrap();
        let path = StorageHandle::File(directory.path().join("stencils.json.zst"));
        let limits = ModelicaArtifactDiskLimits::default();
        let seed = Record {
            version: NATIVE_CODE_CACHE_VERSION,
            entries: vec![
                Entry {
                    key: [1; 32],
                    code: vec![1].into(),
                },
                Entry {
                    key: [2; 32],
                    code: vec![2].into(),
                },
            ],
        };
        FileStorage::new()
            .write_sync(&path, &encode_record(&seed, limits).unwrap())
            .unwrap();
        let mut pool = super::super::thread_pool::ThreadAffinePool::new(
            "native-cache-test",
            1,
            || (),
            |error| panic!("{error}"),
        )
        .unwrap();
        let first = cache_at(&path, pool.next_lane());
        let second = cache_at(&path, pool.next_lane());
        first.load().unwrap();
        second.load().unwrap();
        first.lock().unwrap().insert([3; 32], vec![3].into());
        publish_now(&first);
        let mut stale = Adapter {
            cache: second.clone(),
            fault: RefCell::new(None),
            evictions: Cell::new(0),
        };
        assert_eq!(stale.get(&[1; 32]).unwrap().as_ref(), &[1]);
        stale.insert(&[4; 32], vec![4]);
        publish_now(&second);
        let record = read_record::<Record>(&FileStorage::new(), &path, limits)
            .unwrap()
            .unwrap();
        record.validate().unwrap();
        assert_eq!(
            record
                .entries
                .iter()
                .map(|entry| entry.key)
                .collect::<Vec<_>>(),
            vec![[3; 32], [1; 32], [4; 32]],
            "a stale owner must publish its delta without replacing another owner's code or refreshing untouched entries"
        );
    }

    #[test]
    fn native_code_cache_load_keeps_newer_values_and_pending_recency() {
        let capacities = capacities(2, 100);
        let mut data = Data::new(capacities);
        data.insert([1; 32], vec![9].into());
        data.insert([3; 32], vec![3].into());
        data.get(&[1; 32]).unwrap();
        let revision = data.revision;
        let modified = data.modified;
        data.import_older(
            Record {
                version: NATIVE_CODE_CACHE_VERSION,
                entries: vec![
                    Entry {
                        key: [1; 32],
                        code: vec![1].into(),
                    },
                    Entry {
                        key: [2; 32],
                        code: vec![2].into(),
                    },
                ],
            },
            capacities,
        );
        let delta = data.snapshot(Some(data.published));
        assert_eq!(
            delta.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
            vec![[3; 32], [1; 32]]
        );
        assert_eq!(delta[1].1.as_ref(), &[9]);
        assert_eq!(
            (data.revision, data.modified, data.bytes),
            (revision, modified, 66)
        );
    }

    #[test]
    fn native_code_cache_failed_publication_keeps_pending_changes() {
        let directory = tempfile::tempdir().unwrap();
        let path = StorageHandle::File(directory.path().join("stencils.json.zst"));
        let storage = FileStorage::new();
        let seed = Record {
            version: NATIVE_CODE_CACHE_VERSION,
            entries: vec![Entry {
                key: [1; 32],
                code: vec![1].into(),
            }],
        };
        let encoded = encode_record(&seed, ModelicaArtifactDiskLimits::default()).unwrap();
        storage.write_sync(&path, &encoded).unwrap();
        let mut pool = super::super::thread_pool::ThreadAffinePool::new(
            "native-cache-failure-test",
            1,
            || (),
            |error| panic!("{error}"),
        )
        .unwrap();
        let cache = cache_at(&path, pool.next_lane());
        cache.load().unwrap();
        cache.lock().unwrap().insert([2; 32], vec![2].into());
        storage.write_sync(&path, b"corrupt").unwrap();
        assert!(
            cache
                .publish_delta(vec![([2; 32], vec![2].into())])
                .is_err()
        );
        publish_now(&cache);
        assert_eq!(cache.lock().unwrap().published, 0);
        assert_eq!(storage.read_sync(&path).unwrap(), b"corrupt");
        storage.write_sync(&path, &encoded).unwrap();
        cache.lock().unwrap().insert([3; 32], vec![3].into());
        publish_now(&cache);
        let record = read_record::<Record>(&storage, &path, ModelicaArtifactDiskLimits::default())
            .unwrap()
            .unwrap();
        assert_eq!(
            record
                .entries
                .iter()
                .map(|entry| entry.key)
                .collect::<Vec<_>>(),
            vec![[1; 32], [2; 32], [3; 32]]
        );
    }

    #[test]
    fn native_code_cache_enforces_both_budgets_and_rejects_invalid_records() {
        let mut data = Data::new(capacities(2, 100));
        data.insert([1; 32], vec![1; 8].into());
        data.insert([2; 32], vec![2; 8].into());
        // Replacement refreshes recency and accounts for the previous bytes.
        assert_eq!(data.insert([1; 32], vec![3; 28].into()), 0);
        assert_eq!(data.bytes, 100);
        assert_eq!(data.insert([3; 32], vec![4; 8].into()), 1);
        assert!(!data.entries.contains_key(&[2; 32]));
        assert!(data.entries.contains_key(&[1; 32]));
        assert_eq!(data.bytes, 100);
        assert_eq!(data.insert([4; 32], vec![5; 68].into()), 2);
        assert_eq!(data.entries.len(), 1);
        assert_eq!(data.bytes, 100);
        data.insert([5; 32], vec![6; 69].into());
        assert_eq!(data.entries.len(), 1);
        // The entry budget is independently authoritative.
        data.max_bytes = 1000;
        data.insert([6; 32], vec![7; 1].into());
        assert_eq!(data.insert([7; 32], vec![8; 1].into()), 1);
        assert_eq!(data.entries.len(), 2);
        assert_eq!(data.bytes, 66);
        assert!(!data.entries.contains_key(&[4; 32]));

        let record = Record {
            version: NATIVE_CODE_CACHE_VERSION,
            entries: vec![Entry {
                key: [1; 32],
                code: vec![1].into(),
            }],
        };
        record.validate().unwrap();
        let limits = ModelicaArtifactDiskLimits::default();
        let encoded = encode_record(&record, limits).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let handle = StorageHandle::File(dir.path().join("native.zst"));
        let storage = FileStorage::new();
        storage.write_sync(&handle, &encoded).unwrap();
        let loaded = read_record::<Record>(&storage, &handle, limits)
            .unwrap()
            .unwrap();
        loaded.validate().unwrap();
        assert_eq!(loaded.entries[0].code.as_ref(), &[1]);
        let mut invalid = loaded;
        invalid.entries.push(Entry {
            key: [1; 32],
            code: vec![2].into(),
        });
        assert!(invalid.validate().unwrap_err().contains("duplicate"));
        invalid.entries.pop();
        invalid.entries[0].code = Vec::new().into();
        assert!(invalid.validate().unwrap_err().contains("empty"));
        invalid.version = NATIVE_CODE_CACHE_VERSION + 1;
        assert!(invalid.validate().unwrap_err().contains("version"));
        let mut corrupted = encoded;
        let last = corrupted.len() - 1;
        corrupted[last] ^= 1;
        storage.write_sync(&handle, &corrupted).unwrap();
        assert!(read_record::<Record>(&storage, &handle, limits).is_err());
    }
}
