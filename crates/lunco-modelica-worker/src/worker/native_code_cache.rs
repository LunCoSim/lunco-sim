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
    code: Vec<u8>,
}

impl Record {
    fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
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

struct Data {
    entries: BoundedReuseCache<Key, Arc<[u8]>>,
    bytes: usize,
    max_bytes: usize,
    revision: u64,
    requested: u64,
    attempted: u64,
    writer_active: bool,
}

impl Data {
    fn insert(&mut self, key: Key, code: Arc<[u8]>) -> u64 {
        let weight = key.len() + code.len();
        if code.is_empty() || weight > self.max_bytes {
            return 0;
        }
        let mut evictions = 0;
        loop {
            let replaced = self
                .entries
                .get(&key)
                .map_or(0, |old| key.len() + old.len());
            if self.bytes - replaced + weight <= self.max_bytes {
                break;
            }
            if let Some(old) = self.entries.pop_oldest() {
                self.bytes -= key.len() + old.len();
                evictions += 1;
            }
        }
        let replacing = self.entries.contains_key(&key);
        if let Some(old) = self.entries.insert(key, code) {
            self.bytes -= key.len() + old.len();
            evictions += u64::from(!replacing);
        }
        self.bytes += weight;
        self.revision += 1;
        evictions
    }

    fn snapshot(&self) -> Vec<(Key, Arc<[u8]>)> {
        self.entries
            .ordered_entries()
            .map(|(key, code)| (*key, Arc::clone(code)))
            .collect()
    }
}

#[derive(Clone)]
pub(super) struct SharedNativeCodeCache {
    data: Arc<Mutex<Data>>,
    io: Lane<()>,
    limits: ModelicaArtifactDiskLimits,
    path: StorageHandle,
}

impl SharedNativeCodeCache {
    pub(super) fn new(
        capacities: ModelicaCacheCapacities,
        limits: ModelicaArtifactDiskLimits,
        io: Lane<()>,
    ) -> Result<Self, String> {
        let cache = Self {
            data: Arc::new(Mutex::new(Data {
                entries: BoundedReuseCache::new(capacities.native_code_entries),
                bytes: 0,
                max_bytes: capacities.native_code_bytes.get(),
                revision: 0,
                requested: 0,
                attempted: 0,
                writer_active: false,
            })),
            io,
            limits,
            path: StorageHandle::File(
                lunco_assets_core::modelica_dir()
                    .join("native-code-v1")
                    .join("stencils.json.zst"),
            ),
        };
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
        let Some(record) = read_record::<Record>(&FileStorage::new(), &self.path, self.limits)
            .map_err(|error| error.to_string())?
        else {
            bevy::log::info!("[modelica-runtime] native code artifact cache is cold");
            return Ok(());
        };
        record.validate()?;
        let loaded_entries = record.entries.len();
        let mut data = self.lock()?;
        for entry in record.entries {
            // A concurrent fresh compilation owns its newer value.
            if !data.entries.contains_key(&entry.key) {
                data.insert(entry.key, entry.code.into());
            }
        }
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
            data.requested = data.revision;
            if data.writer_active || data.requested == data.attempted {
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
                Some((revision, data.snapshot()))
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
            let record = Record {
                version: 1,
                entries: entries
                    .into_iter()
                    .map(|(key, code)| Entry {
                        key,
                        code: code.to_vec(),
                    })
                    .collect(),
            };
            let result = encode_record(&record, self.limits)
                .map_err(|error| error.to_string())
                .and_then(|bytes| {
                    FileStorage::new()
                        .write_sync(&self.path, &bytes)
                        .map_err(|error| error.to_string())
                });
            if let Err(error) = result {
                bevy::log::warn!(
                    "[modelica-runtime] native code cache publication failed: {error}"
                );
            }
            match self.lock() {
                Ok(mut data) => {
                    data.attempted = revision;
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
            Ok(data) => data.entries.get(&key).map(|code| Cow::Owned(code.to_vec())),
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
    use std::num::NonZeroUsize;

    #[test]
    fn native_code_cache_enforces_both_budgets_and_rejects_invalid_records() {
        let mut data = Data {
            entries: BoundedReuseCache::new(NonZeroUsize::new(2).unwrap()),
            bytes: 0,
            max_bytes: 100,
            revision: 0,
            requested: 0,
            attempted: 0,
            writer_active: false,
        };
        data.insert([1; 32], vec![1; 8].into());
        data.insert([2; 32], vec![2; 8].into());
        // Replacing the oldest entry must account for its previous bytes.
        assert_eq!(data.insert([1; 32], vec![3; 28].into()), 0);
        assert_eq!(data.bytes, 100);
        assert_eq!(data.insert([3; 32], vec![4; 8].into()), 1);
        assert!(!data.entries.contains_key(&[1; 32]));
        assert_eq!(data.bytes, 80);
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
            version: 1,
            entries: vec![Entry {
                key: [1; 32],
                code: vec![1],
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
        assert_eq!(loaded.entries[0].code, vec![1]);
        let mut invalid = loaded;
        invalid.entries.push(Entry {
            key: [1; 32],
            code: vec![2],
        });
        assert!(invalid.validate().unwrap_err().contains("duplicate"));
        invalid.entries.pop();
        invalid.entries[0].code.clear();
        assert!(invalid.validate().unwrap_err().contains("empty"));
        invalid.version = 2;
        assert!(invalid.validate().unwrap_err().contains("version"));
        let mut corrupted = encoded;
        let last = corrupted.len() - 1;
        corrupted[last] ^= 1;
        storage.write_sync(&handle, &corrupted).unwrap();
        assert!(read_record::<Record>(&storage, &handle, limits).is_err());
    }
}
