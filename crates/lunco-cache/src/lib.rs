//! Generic resource cache with in-flight dedup.
//!
//! # What this solves
//!
//! Three kinds of work recur across our domain crates:
//!
//! 1. **Resolve** — a qualified name / asset path / URI to a concrete
//!    source location (file path, URL).
//! 2. **Load + parse** — read bytes, parse into a domain artifact
//!    (Modelica `StoredDefinition`, USD `Stage`, SysML `Element`).
//! 3. **Share** — multiple consumers want the same artifact without
//!    re-doing the work.
//!
//! Steps 1 and 2 are domain-specific; step 3 is the same every time:
//! a hash map keyed by "resource key" with an in-flight dedup table
//! so N concurrent requests collapse onto one background task.
//! This crate owns step 3 and leaves 1+2 to the [`ResourceLoader`]
//! impl each domain provides.
//!
//! # Example sketch
//!
//! ```ignore
//! struct MyLoader;
//! impl lunco_cache::ResourceLoader for MyLoader {
//!     type Key = String;
//!     type Value = Vec<u8>;
//!     type Error = std::io::Error;
//!     fn load(&self, key: &String) -> Task<Result<Vec<u8>, std::io::Error>> {
//!         let path = key.clone();
//!         bevy::tasks::AsyncComputeTaskPool::get()
//!             .spawn(async move { std::fs::read(&path) })
//!     }
//! }
//!
//! let mut cache = lunco_cache::ResourceCache::new(MyLoader);
//! cache.request("/tmp/foo.bin".into());
//! // ... from a Bevy system, once per frame:
//! cache.drive();
//! // ... anywhere:
//! if let Some(bytes) = cache.peek(&"/tmp/foo.bin".into()) {
//!     /* use bytes */
//! }
//! ```
//!
//! # Why bevy-flavored tasks
//!
//! The cache is a long-lived resource polled from a Bevy `Update`
//! system. Using `bevy::tasks::Task<T>` as the pending-handle type
//! keeps the "spawn side" (whichever pool the loader picks —
//! `AsyncComputeTaskPool` for I/O + parse, `ComputeTaskPool` for
//! CPU-heavy transforms) and the "poll side" (cache's `drive`)
//! decoupled without a separate executor dependency.

use bevy::prelude::Resource;
use bevy::tasks::Task;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;

/// A domain-specific recipe for turning a `Key` into a `Value`.
///
/// The cache calls [`ResourceLoader::load`] at most once per miss
/// per key (subsequent `request`s for an in-flight or already-ready
/// key are no-ops). Implementations typically spawn via
/// `bevy::tasks::AsyncComputeTaskPool::get().spawn(...)` so the
/// load runs off the UI thread.
pub trait ResourceLoader: Send + Sync + 'static {
    /// Name / path / URI that uniquely identifies a resource.
    type Key: Eq + Hash + Clone + Send + Sync + std::fmt::Debug + 'static;
    /// The parsed artifact consumers use.
    type Value: Send + Sync + 'static;
    /// Load failure. Stored as a stringified `Display` in the cache so
    /// consumers aren't forced to carry the error type around.
    type Error: std::fmt::Display + Send + 'static;

    fn load(&self, key: &Self::Key) -> Task<Result<Self::Value, Self::Error>>;
}

/// Terminal state of a cache entry. The `Arc` lets many readers share
/// a parsed artifact without cloning heavyweight data (ASTs, scene
/// graphs).
#[derive(Clone)]
pub enum ResourceState<V> {
    Ready(Arc<V>),
    Failed(Arc<str>),
}

/// The cache itself. One instance per `ResourceLoader` kind per App;
/// typically wrapped in a domain-specific newtype (e.g. Modelica's
/// `ClassCache(ResourceCache<ModelicaClassLoader>)`).
#[derive(Resource)]
pub struct ResourceCache<L: ResourceLoader> {
    loader: L,
    entries: HashMap<L::Key, ResourceState<L::Value>>,
    pending: HashMap<L::Key, Task<Result<L::Value, L::Error>>>,
}

impl<L: ResourceLoader> ResourceCache<L> {
    pub fn new(loader: L) -> Self {
        Self {
            loader,
            entries: HashMap::new(),
            pending: HashMap::new(),
        }
    }

    /// Non-blocking read of a ready entry. Returns `None` for misses
    /// AND for in-flight loads — use [`is_loading`] to distinguish.
    pub fn peek(&self, key: &L::Key) -> Option<Arc<L::Value>> {
        match self.entries.get(key) {
            Some(ResourceState::Ready(v)) => Some(Arc::clone(v)),
            _ => None,
        }
    }

    pub fn is_loading(&self, key: &L::Key) -> bool {
        self.pending.contains_key(key)
    }

    pub fn state(&self, key: &L::Key) -> Option<&ResourceState<L::Value>> {
        self.entries.get(key)
    }

    /// Start a load if this key isn't already ready or in flight.
    /// Returns `true` if a new task was spawned. Safe to call from
    /// hot paths — cache-hit case is a single HashMap probe.
    pub fn request(&mut self, key: L::Key) -> bool {
        if self.entries.contains_key(&key) || self.pending.contains_key(&key) {
            return false;
        }
        let task = self.loader.load(&key);
        self.pending.insert(key, task);
        true
    }

    /// Poll all pending tasks. Call once per frame from a Bevy
    /// `Update` system. Returns the keys that resolved this tick so
    /// callers can fire follow-up work (e.g. drill-in → install into
    /// registry, or UI repaint requests).
    pub fn drive(&mut self) -> Vec<L::Key> {
        use futures_lite::future;
        let mut resolved = Vec::new();
        // Snapshot keys so we can mutate `pending` inside the loop.
        let keys: Vec<L::Key> = self.pending.keys().cloned().collect();
        for key in keys {
            let Some(task) = self.pending.get_mut(&key) else {
                continue;
            };
            let Some(result) = future::block_on(future::poll_once(task)) else {
                continue;
            };
            self.pending.remove(&key);
            let state = match result {
                Ok(v) => ResourceState::Ready(Arc::new(v)),
                Err(e) => ResourceState::Failed(e.to_string().into()),
            };
            self.entries.insert(key.clone(), state);
            resolved.push(key);
        }
        resolved
    }

    /// Drop a ready entry. In-flight loads for the same key are NOT
    /// cancelled — if you evict while a task is running, its result
    /// will install on the next `drive` tick. Callers that care can
    /// check [`is_loading`] before evicting.
    pub fn evict(&mut self, key: &L::Key) -> bool {
        self.entries.remove(key).is_some()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Access the loader (used by tests and diagnostics). Most code
    /// should go through [`request`] / [`peek`] instead.
    pub fn loader(&self) -> &L {
        &self.loader
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::tasks::AsyncComputeTaskPool;

    struct EchoLoader;
    impl ResourceLoader for EchoLoader {
        type Key = String;
        type Value = String;
        type Error = std::io::Error;
        fn load(&self, key: &String) -> Task<Result<String, std::io::Error>> {
            let k = key.clone();
            AsyncComputeTaskPool::get().spawn(async move { Ok(format!("loaded:{k}")) })
        }
    }

    #[test]
    fn request_dedupes_concurrent_calls() {
        // Bevy's AsyncComputeTaskPool needs initialization when tests
        // run standalone; if it's missing we skip (the CI app-level
        // test harness does the init).
        if AsyncComputeTaskPool::try_get().is_none() {
            AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
        }
        let mut cache = ResourceCache::new(EchoLoader);
        assert!(cache.request("foo".into()));
        assert!(!cache.request("foo".into()), "second request should dedupe");
        // Drain; busy-loop a bounded number of ticks since the task
        // pool in a unit test is fine but deterministic timing isn't.
        for _ in 0..100 {
            let resolved = cache.drive();
            if !resolved.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let got = cache.peek(&"foo".into()).expect("should be ready");
        assert_eq!(&*got, "loaded:foo");
    }
}
