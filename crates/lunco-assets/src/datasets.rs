//! Downloadable **datasets** — the runtime half of `Assets.toml`.
//!
//! [`download`](crate::download) knows how to fetch one declared entry; this
//! module is where a running app asks for that, tracks it, and answers "what is
//! downloadable, and what state is it in?".
//!
//! # The rule this module exists to enforce
//!
//! **The app never reaches the network on its own.** Launching, loading a
//! scene, or opening a twin must not open a connection. Anything fetchable is
//! *declared* in an `Assets.toml`, listed here, and downloaded only when a user
//! explicitly asks. That is why the fetch lives in this crate rather than in
//! each consumer: a domain crate that owns its own downloader inevitably grows
//! a "just fetch it at startup" line, and the guarantee dies one crate at a
//! time.
//!
//! # Division of labour
//!
//! - **This crate** — owns the manifest, the URL, the cache path, the task, the
//!   bytes, and the status.
//! - **A domain crate** (ephemeris, MSL, terrain, …) — declares its datasets in
//!   its own `Assets.toml`, registers that manifest here, and *reports* what it
//!   did with the file (loaded / not loaded). It never builds a URL and never
//!   opens a socket.
//! - **A UI** — renders [`DatasetRegistry::entries`] and calls
//!   [`DatasetRegistry::request`]. It needs no per-dataset knowledge.
//!
//! # Registering
//!
//! Manifests are embedded, not read from the source tree — a packaged binary
//! has no `crates/…/Assets.toml`:
//!
//! ```ignore
//! app.add_plugins(lunco_assets::datasets::DatasetsPlugin);
//! // in your plugin's build():
//! world.resource_mut::<DatasetRegistry>()
//!     .register(include_str!("../Assets.toml"), "ephemeris");
//! ```

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bevy::prelude::*;

use crate::download::{entry_dest_path, AssetEntry, AssetManifest};

/// What a declared dataset is currently doing.
#[derive(Debug, Clone, PartialEq)]
pub enum DatasetState {
    /// Declared, not on disk. Nothing has been fetched.
    Missing,
    /// A user-requested download is running. `total == 0` when the server
    /// sends no length.
    Downloading {
        /// Bytes received so far.
        bytes_done: u64,
        /// Bytes expected, or `0` when unknown.
        bytes_total: u64,
    },
    /// The file is on disk at its declared destination.
    Installed,
    /// The last download attempt failed; the message is the reason.
    Failed(String),
}

impl DatasetState {
    /// Whether the bytes are available locally right now.
    pub fn is_installed(&self) -> bool {
        matches!(self, DatasetState::Installed)
    }
}

/// Who declared a dataset, which decides WHERE its bytes land.
#[derive(Debug, Clone, PartialEq)]
pub enum DatasetScope {
    /// Declared by the engine (a crate's own `Assets.toml`) → the shared
    /// [`cache_dir`](crate::cache_dir).
    Engine,
    /// Declared by a Twin → that Twin's own cache
    /// ([`twin_cache_dir`](crate::twin_cache_dir)), so the data travels and
    /// dies with the folder.
    Twin {
        /// `twin://` authority the root is registered under.
        name: String,
        /// Absolute Twin root.
        root: PathBuf,
    },
}

impl DatasetScope {
    /// The directory a scoped entry's `dest` resolves against.
    pub fn dest_root(&self) -> PathBuf {
        match self {
            DatasetScope::Engine => crate::cache_dir(),
            DatasetScope::Twin { root, .. } => crate::twin_cache_dir(root),
        }
    }

    /// Label for UI grouping.
    pub fn label(&self) -> &str {
        match self {
            DatasetScope::Engine => "engine",
            DatasetScope::Twin { name, .. } => name,
        }
    }
}

/// One declared dataset, plus where it lives and how it's doing.
#[derive(Debug, Clone)]
pub struct DatasetEntry {
    /// Manifest key (`[artemis2_vectors]` → `"artemis2_vectors"`), unique
    /// within its scope.
    pub key: String,
    /// Which registrant declared it — shown in UI groupings, e.g. `"ephemeris"`.
    pub group: String,
    /// Engine-declared or Twin-declared; decides the destination cache.
    pub scope: DatasetScope,
    /// Human-readable name from the manifest.
    pub name: String,
    /// Where the file lands once downloaded.
    pub path: PathBuf,
    /// Live status.
    pub state: DatasetState,
    /// The full declaration, so the crate that owns this dataset can read its
    /// own domain sub-table ([`AssetEntry::domain`]) — for an engine manifest
    /// and a Twin's alike, without either of them re-reading the file.
    pub spec: AssetEntry,
}

/// Cross-thread slot a download task writes its progress into.
type StatusSlot = Arc<Mutex<Option<DatasetState>>>;

/// Every dataset any crate has declared, and its live state.
///
/// Registration order is irrelevant; keys are unique, and a duplicate key is
/// refused rather than silently overwriting another crate's dataset.
#[derive(Resource, Default)]
pub struct DatasetRegistry {
    entries: Vec<DatasetEntry>,
    /// Per-entry status slot, written by the task, drained in `Update`.
    slots: Vec<StatusSlot>,
}

impl DatasetRegistry {
    /// Register every entry of an embedded `Assets.toml` as ENGINE-scoped
    /// (destination: the shared cache).
    ///
    /// Returns the number of entries added. A malformed manifest is reported
    /// and contributes nothing — a broken declaration must not take the app
    /// down, and it must not be silent either.
    pub fn register(&mut self, assets_toml: &str, group: &str) -> usize {
        self.register_scoped(assets_toml, group, DatasetScope::Engine)
    }

    /// Register a manifest under an explicit [`DatasetScope`].
    pub fn register_scoped(
        &mut self,
        assets_toml: &str,
        group: &str,
        scope: DatasetScope,
    ) -> usize {
        let manifest: AssetManifest = match assets_toml.parse() {
            Ok(m) => m,
            Err(e) => {
                error!("[datasets] {group}: Assets.toml parse failed: {e}");
                return 0;
            }
        };
        let dest_root = scope.dest_root();
        let mut added = 0;
        for (key, entry) in manifest.assets {
            // Keys are unique PER SCOPE: two Twins may both declare `dtm`, and
            // neither may shadow the other or the engine's.
            if self
                .entries
                .iter()
                .any(|e| e.key == key && e.scope == scope)
            {
                error!(
                    "[datasets] duplicate dataset key '{key}' within scope '{}' — ignored",
                    scope.label()
                );
                continue;
            }
            let path = entry_dest_path(&entry, Some(&dest_root));
            self.entries.push(DatasetEntry {
                key: key.clone(),
                group: group.to_string(),
                scope: scope.clone(),
                name: entry.name.clone(),
                // Present on disk ⇒ installed, whoever put it there (a previous
                // run, the CLI downloader, a hand-copied file). The registry
                // reports the filesystem, it doesn't own a separate truth.
                state: if path.exists() {
                    DatasetState::Installed
                } else {
                    DatasetState::Missing
                },
                path,
                spec: entry,
            });
            self.slots.push(Arc::new(Mutex::new(None)));
            added += 1;
        }
        added
    }

    /// Scan an opened Twin folder for its `Assets.toml` and register what it
    /// declares, Twin-scoped. Idempotent per (twin root, key): reopening a Twin
    /// re-reads the manifest and refreshes on-disk state without duplicating
    /// rows.
    ///
    /// The manifest is read from disk here — unlike a crate's, a Twin's
    /// manifest is user data that changes while the app runs, so embedding it
    /// would be a lie.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn scan_twin(&mut self, name: &str, root: &std::path::Path) -> usize {
        let scope = DatasetScope::Twin {
            name: name.to_string(),
            root: root.to_path_buf(),
        };
        self.forget_scope(&scope);
        let manifest_path = root.join("Assets.toml");
        let Ok(text) = std::fs::read_to_string(&manifest_path) else {
            return 0; // A Twin without a manifest declares no datasets.
        };
        let n = self.register_scoped(&text, name, scope);
        if n > 0 {
            info!("[datasets] twin '{name}': {n} declared dataset(s)");
        }
        n
    }

    /// Drop every entry of a scope (a Twin closing, or a rescan).
    /// In-flight downloads for dropped entries finish into a slot nobody
    /// reads, which is the honest outcome: their bytes still land on disk and
    /// the next scan reports them installed.
    pub fn forget_scope(&mut self, scope: &DatasetScope) {
        let mut i = 0;
        while i < self.entries.len() {
            if &self.entries[i].scope == scope {
                self.entries.remove(i);
                self.slots.remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// Re-read on-disk presence for every entry. Cheap (`Path::exists` per
    /// dataset) and only meaningful for entries not currently downloading.
    pub fn refresh_installed_state(&mut self) {
        for e in &mut self.entries {
            if matches!(e.state, DatasetState::Downloading { .. }) {
                continue;
            }
            e.state = if e.path.exists() {
                DatasetState::Installed
            } else if let DatasetState::Failed(msg) = &e.state {
                DatasetState::Failed(msg.clone())
            } else {
                DatasetState::Missing
            };
        }
    }

    /// Every declared dataset, in registration order.
    pub fn entries(&self) -> &[DatasetEntry] {
        &self.entries
    }

    /// State of one dataset, or `None` if nothing declared that key.
    pub fn state(&self, key: &str) -> Option<&DatasetState> {
        self.entries.iter().find(|e| e.key == key).map(|e| &e.state)
    }

    /// On-disk path of one dataset, or `None` if nothing declared that key.
    pub fn path(&self, key: &str) -> Option<&std::path::Path> {
        self.entries
            .iter()
            .find(|e| e.key == key)
            .map(|e| e.path.as_path())
    }

    /// Datasets that are declared but not on disk.
    pub fn missing(&self) -> impl Iterator<Item = &DatasetEntry> {
        self.entries
            .iter()
            .filter(|e| matches!(e.state, DatasetState::Missing | DatasetState::Failed(_)))
    }

    /// Start downloading `key`. **The only call in the engine that authorises
    /// network traffic for declared assets** — wire it to an explicit user
    /// action, never to startup or scene load.
    ///
    /// No-op when the dataset is already installed or already downloading.
    pub fn request(&mut self, key: &str) {
        let Some(i) = self.entries.iter().position(|e| e.key == key) else {
            warn!("[datasets] request for unknown dataset '{key}'");
            return;
        };
        if matches!(
            self.entries[i].state,
            DatasetState::Installed | DatasetState::Downloading { .. }
        ) {
            return;
        }
        self.entries[i].state = DatasetState::Downloading {
            bytes_done: 0,
            bytes_total: 0,
        };
        let dest_root = self.entries[i].scope.dest_root();
        let spec = self.entries[i].spec.clone();
        spawn_download(&self.entries[i], &spec, dest_root, self.slots[i].clone());
    }

    /// Start every missing dataset. Same authorisation rule as [`request`](Self::request).
    pub fn request_all_missing(&mut self) {
        let keys: Vec<String> = self.missing().map(|e| e.key.clone()).collect();
        for k in keys {
            self.request(&k);
        }
    }
}

/// Spawn the actual fetch on the async pool.
#[cfg(not(target_arch = "wasm32"))]
fn spawn_download(entry: &DatasetEntry, spec: &AssetEntry, dest_root: PathBuf, slot: StatusSlot) {
    use crate::download::{download_asset_with_control, DownloadControl};

    let key = entry.key.clone();
    let name = entry.name.clone();
    let spec = spec.clone();
    let progress_slot = slot.clone();
    info!("[datasets] downloading '{key}' ({name}) — user-requested");
    bevy::tasks::AsyncComputeTaskPool::get()
        .spawn(async move {
            let control = DownloadControl {
                progress: Some(Box::new(move |done, total| {
                    if let Ok(mut s) = progress_slot.lock() {
                        *s = Some(DatasetState::Downloading {
                            bytes_done: done,
                            bytes_total: total,
                        });
                    }
                })),
                extracting: None,
                cancel: None,
            };
            // The scope decided the root: engine → shared cache, twin →
            // `<twin>/.cache`. Same resolver the CLI downloader uses, so a
            // file fetched from the app and one fetched from the terminal land
            // in exactly the same place.
            let outcome = match download_asset_with_control(
                &spec,
                &key,
                control,
                Some(dest_root.as_path()),
            ) {
                Ok(()) => DatasetState::Installed,
                Err(e) => DatasetState::Failed(e.to_string()),
            };
            if let Ok(mut s) = slot.lock() {
                *s = Some(outcome);
            }
        })
        .detach();
}

/// The web build has no cache directory to fill and no HTTP downloader here;
/// its assets are served by the host. Requesting is a reported no-op rather
/// than a silent one.
#[cfg(target_arch = "wasm32")]
fn spawn_download(
    entry: &DatasetEntry,
    _spec: &AssetEntry,
    _dest_root: PathBuf,
    slot: StatusSlot,
) {
    warn!(
        "[datasets] '{}' cannot be downloaded in the browser build — it is served by the host",
        entry.key
    );
    if let Ok(mut s) = slot.lock() {
        *s = Some(DatasetState::Failed("not downloadable on web".into()));
    }
}

/// Drain task-written status into the registry. Cheap: one `try_lock` per
/// dataset, and only while something is in flight.
fn drain_dataset_status(registry: Option<ResMut<DatasetRegistry>>) {
    let Some(mut registry) = registry else { return };
    if !registry
        .entries
        .iter()
        .any(|e| matches!(e.state, DatasetState::Downloading { .. }))
    {
        return;
    }
    for i in 0..registry.entries.len() {
        let next = registry.slots[i].lock().ok().and_then(|mut s| s.take());
        if let Some(state) = next {
            if let DatasetState::Failed(ref e) = state {
                warn!("[datasets] '{}' failed: {e}", registry.entries[i].key);
            }
            if state.is_installed() {
                info!("[datasets] '{}' installed", registry.entries[i].key);
            }
            registry.entries[i].state = state;
        }
    }
}

/// Keep the registry in step with the set of OPEN Twins.
///
/// A Twin's datasets are its own; they appear when it opens and go when it
/// closes. Registration therefore cannot be a startup act — it follows
/// [`TwinRoots`](crate::TwinRoots), which is mutated through interior
/// mutability (no Bevy change detection), so the honest check is to diff the
/// name set. That is a lock plus a small `Vec<String>` per frame, against a
/// registry that is at most a handful of Twins.
#[cfg(not(target_arch = "wasm32"))]
fn scan_open_twins_for_datasets(
    roots: Option<Res<crate::TwinRoots>>,
    registry: Option<ResMut<DatasetRegistry>>,
) {
    let (Some(roots), Some(mut registry)) = (roots, registry) else {
        return;
    };
    let open = roots.names();

    // Gone: forget every scope whose Twin is no longer open.
    let stale: Vec<DatasetScope> = registry
        .entries
        .iter()
        .filter_map(|e| match &e.scope {
            DatasetScope::Twin { name, .. } if !open.contains(name) => Some(e.scope.clone()),
            _ => None,
        })
        .collect();
    for scope in stale {
        registry.forget_scope(&scope);
    }

    // New: scan any open Twin the registry has not seen.
    for name in open {
        let known = registry
            .entries
            .iter()
            .any(|e| matches!(&e.scope, DatasetScope::Twin { name: n, .. } if *n == name));
        if known {
            continue;
        }
        if let Some(root) = roots.root_for(&name) {
            registry.scan_twin(&name, &root);
        }
    }
}

/// Adds the [`DatasetRegistry`], its status pump, and the open-Twin scan.
/// Idempotent.
pub struct DatasetsPlugin;

impl Plugin for DatasetsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DatasetRegistry>();
        app.add_systems(Update, drain_dataset_status);
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Update, scan_open_twins_for_datasets);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"
[demo_vectors]
name = "Demo vectors"
url = "https://example.invalid/vectors.csv"
dest = "ephemeris/demo.csv"
"#;

    #[test]
    fn registering_a_manifest_lists_it_as_missing_not_fetched() {
        let mut r = DatasetRegistry::default();
        assert_eq!(r.register(MANIFEST, "demo"), 1);
        let e = &r.entries()[0];
        assert_eq!(e.key, "demo_vectors");
        assert_eq!(e.group, "demo");
        // The point of the module: declaring a dataset must never fetch it.
        assert_eq!(e.state, DatasetState::Missing);
        assert!(e.path.ends_with("ephemeris/demo.csv"));
    }

    #[test]
    fn duplicate_keys_are_refused_not_overwritten() {
        let mut r = DatasetRegistry::default();
        assert_eq!(r.register(MANIFEST, "first"), 1);
        assert_eq!(r.register(MANIFEST, "second"), 0);
        assert_eq!(r.entries().len(), 1);
        assert_eq!(r.entries()[0].group, "first");
    }

    #[test]
    fn a_broken_manifest_contributes_nothing() {
        let mut r = DatasetRegistry::default();
        assert_eq!(r.register("this is not toml {{{", "bad"), 0);
        assert!(r.entries().is_empty());
    }

    #[test]
    fn unknown_key_lookups_are_none() {
        let r = DatasetRegistry::default();
        assert!(r.state("nope").is_none());
        assert!(r.path("nope").is_none());
    }
}
