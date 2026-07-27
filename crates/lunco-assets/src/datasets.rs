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
    /// The directory a scoped entry's `dest` resolves against — where a
    /// download WRITES.
    pub fn dest_root(&self) -> PathBuf {
        match self {
            DatasetScope::Engine => crate::cache_dir(),
            DatasetScope::Twin { root, .. } => crate::twin_cache_dir(root),
        }
    }

    /// Every directory a scoped entry's file may be READ from, in priority
    /// order. Wider than [`dest_root`](Self::dest_root) on purpose: bytes that
    /// arrived with a distribution were never written by this machine.
    ///
    /// An engine dataset may ship inside the package (`assets/.cache`) rather
    /// than sitting in the machine's pool; a Twin's may arrive in the `.cache`
    /// of an archive someone sent. Both are installed — asking only where WE
    /// would have written would report them missing and offer to re-download a
    /// file already on disk.
    pub fn read_roots(&self) -> Vec<PathBuf> {
        match self {
            DatasetScope::Engine => crate::cache_roots(),
            DatasetScope::Twin { root, .. } => {
                vec![crate::twin_cache_dir(root), root.clone()]
            }
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
    /// The file this dataset actually DELIVERS, relative to its scope root:
    /// the `[*.process]` output when the declaration has one, else the download
    /// itself.
    ///
    /// The two differ for anything derived — Earth's manifest downloads a 5400×
    /// JPEG and delivers a 4096×2048 PNG. Reporting "installed" off the
    /// *download* would call a dataset ready while the file every consumer
    /// actually loads was still missing, which is precisely the state a user
    /// cannot distinguish from a broken texture.
    ///
    /// Relative, not absolute, because the same relative path is where the file
    /// is WRITTEN (under [`DatasetScope::dest_root`]) and where it may already
    /// be found (under any [`DatasetScope::read_roots`]) — see
    /// [`artifact_path`](Self::artifact_path) and [`artifact_uri`](Self::artifact_uri).
    pub artifact_rel: String,
    /// Live status.
    pub state: DatasetState,
    /// The full declaration, so the crate that owns this dataset can read its
    /// own domain sub-table ([`AssetEntry::domain`]) — for an engine manifest
    /// and a Twin's alike, without either of them re-reading the file.
    pub spec: AssetEntry,
}

/// Whether a path holds bytes we can read *locally*.
///
/// Always `false` on wasm: the browser build has no filesystem (`Path::exists`
/// panics there with "no filesystem on this platform"), and its assets are
/// served by the host over HTTP rather than installed. So a web build reports
/// every dataset missing, which is the honest answer — it cannot install one
/// either.
fn present(path: &std::path::Path) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
        false
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        path.exists()
    }
}

impl DatasetEntry {
    /// Absolute path of the delivered file: the first
    /// [`read root`](DatasetScope::read_roots) that actually holds it, else the
    /// root a download would write it to.
    pub fn artifact_path(&self) -> PathBuf {
        let roots = self.scope.read_roots();
        for root in &roots {
            let candidate = root.join(&self.artifact_rel);
            if present(&candidate) {
                return candidate;
            }
        }
        self.scope.dest_root().join(&self.artifact_rel)
    }

    /// The asset URI the delivered file loads at — `lunco://<rel>` for an
    /// engine dataset, `twin://<name>/<rel>` for a Twin's.
    ///
    /// Both schemes already search their own cache before falling through, so
    /// a consumer never learns which root the bytes came from. That is what
    /// lets one URI mean "packaged copy" on a distributed build and "freshly
    /// downloaded" on a dev machine, with no branch at the call site.
    pub fn artifact_uri(&self) -> String {
        match &self.scope {
            DatasetScope::Engine => crate::asset_path::uri(crate::LUNCO_SCHEME, &self.artifact_rel),
            DatasetScope::Twin { name, .. } => crate::twin_uri(name, &self.artifact_rel),
        }
    }
}

/// Scope-root-relative path of the file a declaration ultimately delivers — its
/// `[*.process]` output where there is one, else the download destination.
///
/// Twin scope hands the process resolver BOTH roots it distinguishes: the
/// Twin's `.cache` for derived artifacts, the Twin folder for authored ones.
fn artifact_rel_of(entry: &AssetEntry, scope: &DatasetScope, dest: &std::path::Path) -> String {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(cfg) = &entry.process {
        let twin_root = match scope {
            DatasetScope::Twin { root, .. } => Some(root.as_path()),
            DatasetScope::Engine => None,
        };
        let cache_root = scope.dest_root();
        let abs = crate::process::process_output_path(cfg, Some(&cache_root), twin_root);
        // `output_root = "assets"` writes into the source tree, which no scope
        // root contains; the declared `output` is already the right relative
        // spelling there, so fall back to it rather than to an absolute path
        // no reader could resolve.
        return abs
            .strip_prefix(&cache_root)
            .map(crate::asset_path::slashed)
            .unwrap_or_else(|_| cfg.output.clone());
    }
    let _ = entry;
    dest.strip_prefix(scope.dest_root())
        .map(crate::asset_path::slashed)
        .unwrap_or_else(|_| crate::asset_path::slashed(dest))
}

/// Cross-thread slot a download task writes its progress into.
type StatusSlot = Arc<Mutex<Option<DatasetState>>>;

/// Telemetry event name published when a declared dataset cannot be offered or
/// cannot be fetched: an unparseable `Assets.toml`, a manifest file that will
/// not read, a duplicate key, or a failed download.
///
/// Published at [`Severity::Error`](lunco_core::Severity::Error) so the status
/// bar's error-telemetry observer surfaces it — the same arrangement
/// `lunco_usd_bevy::SCENE_LOAD_FAILED` and `lunco_tutorial::TUTORIAL_FAILED`
/// use. A user whose dataset panel is empty because a manifest is broken must
/// be told WHY in the UI, not only in a terminal they are not watching.
/// The payload is a human-readable cause string.
pub const DATASET_FAILED: &str = "DATASET_FAILED";

/// The one shape every [`DATASET_FAILED`] publication takes, so the payload
/// convention cannot drift between sites.
fn dataset_failed(detail: impl Into<String>) -> lunco_core::TelemetryEvent {
    lunco_core::TelemetryEvent {
        name: DATASET_FAILED.into(),
        source: 0,
        severity: lunco_core::Severity::Error,
        data: lunco_core::TelemetryValue::String(detail.into()),
        timestamp: 0.0,
    }
}

/// Everything one download ATTEMPT communicates with: the status slot it writes
/// into, and the flag that tells it to give up.
///
/// One handle per attempt, not per entry. A task we stopped waiting for (stall
/// watchdog, explicit cancel) is detached and may still be inside a blocking
/// read; if it shared a slot with the next attempt its late verdict would
/// clobber the live one. Replacing the handle orphans it harmlessly — it writes
/// into a slot nobody reads, exactly as [`DatasetRegistry::forget_scope`]
/// already relies on.
struct DownloadHandle {
    status: StatusSlot,
    cancel: Arc<std::sync::atomic::AtomicBool>,
}

impl Default for DownloadHandle {
    fn default() -> Self {
        Self {
            status: Arc::new(Mutex::new(None)),
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
}

/// Every dataset any crate has declared, and its live state.
///
/// Registration order is irrelevant; keys are unique, and a duplicate key is
/// refused rather than silently overwriting another crate's dataset.
#[derive(Resource, Default)]
pub struct DatasetRegistry {
    entries: Vec<DatasetEntry>,
    /// Per-entry download handle, written by the task, drained in `Update`.
    slots: Vec<DownloadHandle>,
    /// Failures raised from `&mut self` methods, which have no `Commands`.
    /// Drained into [`DATASET_FAILED`] telemetry by [`drain_dataset_status`].
    pending_failures: Vec<String>,
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
                // The log line is not the user's channel: a broken manifest
                // means the dataset panel silently offers nothing, which is
                // indistinguishable from "nothing is declared".
                self.pending_failures
                    .push(format!("{group}: Assets.toml parse failed: {e}"));
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
                self.pending_failures.push(format!(
                    "duplicate dataset key '{key}' within scope '{}' — ignored",
                    scope.label()
                ));
                continue;
            }
            let path = entry_dest_path(&entry, Some(&dest_root));
            let artifact_rel = artifact_rel_of(&entry, &scope, &path);
            let installed = scope
                .read_roots()
                .iter()
                .any(|r| present(&r.join(&artifact_rel)));
            self.entries.push(DatasetEntry {
                key: key.clone(),
                group: group.to_string(),
                scope: scope.clone(),
                name: entry.name.clone(),
                // Present on disk ⇒ installed, whoever put it there (a previous
                // run, the CLI downloader, an archive a colleague sent). The
                // registry reports the filesystem, it doesn't own a separate
                // truth — which is what makes a Twin unpacked WITH its `.cache`
                // simply show up installed, no re-download and no import step.
                state: if installed {
                    DatasetState::Installed
                } else {
                    DatasetState::Missing
                },
                path,
                artifact_rel,
                spec: entry,
            });
            self.slots.push(DownloadHandle::default());
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
            e.state = if present(&e.artifact_path()) {
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

    /// On-disk path of the file one dataset DELIVERS (its `[*.process]` output
    /// where it has one), or `None` if nothing declared that key. This is the
    /// path a consumer loads; [`DatasetEntry::path`] is where the download
    /// landed, which for a derived product is not the same file.
    pub fn path(&self, key: &str) -> Option<PathBuf> {
        self.entries
            .iter()
            .find(|e| e.key == key)
            .map(|e| e.artifact_path())
    }

    /// The installed dataset delivering `key`, or `None` when it is not
    /// declared or not on disk. The one call a consumer needs: "are these bytes
    /// available, and where?".
    pub fn installed(&self, key: &str) -> Option<&DatasetEntry> {
        self.entries
            .iter()
            .find(|e| e.key == key && e.state.is_installed())
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
    ///
    /// `Downloading` is not a sticky state: every attempt is watched by a stall
    /// watchdog that turns a wedged transfer into [`DatasetState::Failed`], and
    /// `Failed` is requestable again — so "the host went away mid-download"
    /// costs the user a wait, never the process lifetime.
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
        // Fresh handle per attempt — see `DownloadHandle`. A retry after a
        // stall must not be able to inherit the abandoned task's verdict.
        self.slots[i] = DownloadHandle::default();
        let scope = self.entries[i].scope.clone();
        let spec = self.entries[i].spec.clone();
        spawn_download(
            &self.entries[i],
            &spec,
            scope,
            self.slots[i].status.clone(),
            self.slots[i].cancel.clone(),
        );
    }

    /// Give up on an in-flight download: raise the task's cancel flag and put
    /// the entry straight into [`DatasetState::Failed`] so it is requestable
    /// again immediately.
    ///
    /// The state does not wait for the task to notice. A task parked in a
    /// blocking socket read cannot answer until bytes arrive or the OS gives
    /// up, and holding the UI hostage to that is exactly the wedge this exists
    /// to prevent — so the entry is released now and the doomed task, whose
    /// handle has been replaced, writes into a slot nobody reads.
    pub fn cancel(&mut self, key: &str) {
        let Some(i) = self.entries.iter().position(|e| e.key == key) else {
            warn!("[datasets] cancel for unknown dataset '{key}'");
            return;
        };
        if !matches!(self.entries[i].state, DatasetState::Downloading { .. }) {
            return;
        }
        self.slots[i]
            .cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.slots[i] = DownloadHandle::default();
        self.entries[i].state = DatasetState::Failed("cancelled".into());
        info!("[datasets] '{key}' download cancelled by user");
    }

    /// Start every missing dataset. Same authorisation rule as [`request`](Self::request).
    pub fn request_all_missing(&mut self) {
        let keys: Vec<String> = self.missing().map(|e| e.key.clone()).collect();
        for k in keys {
            self.request(&k);
        }
    }
}

/// How often the stall watchdog samples the liveness counter. Small relative to
/// [`BODY_STALL_TIMEOUT`](crate::download::BODY_STALL_TIMEOUT) so the reported
/// idle time is accurate to a couple of seconds, large enough that watching a
/// download costs nothing.
#[cfg(not(target_arch = "wasm32"))]
const STALL_POLL: std::time::Duration = std::time::Duration::from_secs(2);

/// Spawn the actual fetch on the async pool, plus the watchdog that gives it a
/// deadline.
///
/// Two tasks, because the fetch cannot time itself: it spends its life inside a
/// blocking `read` that returns when the peer feels like it. The watchdog
/// watches a liveness counter instead — an INACTIVITY test, not a
/// total-duration cap, so an hour-long GeoTIFF over a slow link is fine and a
/// host that stops sending is not.
#[cfg(not(target_arch = "wasm32"))]
fn spawn_download(
    entry: &DatasetEntry,
    spec: &AssetEntry,
    scope: DatasetScope,
    slot: StatusSlot,
    cancel: Arc<std::sync::atomic::AtomicBool>,
) {
    use crate::download::{download_asset_with_control, DownloadControl, BODY_STALL_TIMEOUT};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    let key = entry.key.clone();
    let name = entry.name.clone();
    let spec = spec.clone();
    let dest = entry.path.clone();
    let dest_root = scope.dest_root();
    let progress_slot = slot.clone();

    // Shared with the watchdog: a monotonic LIVENESS counter bumped by every
    // progress tick (a chunk read) and every extraction tick, plus the flag
    // that retires the watchdog once the fetch returns.
    //
    // A counter rather than the byte total, because unpacking a tarball is
    // also progress and reports no bytes — watching the byte count alone would
    // declare a healthy 2-minute extraction stalled.
    let activity = Arc::new(AtomicU64::new(0));
    let network_done = Arc::new(AtomicBool::new(false));

    info!("[datasets] downloading '{key}' ({name}) — user-requested");

    {
        // Watchdog. Retired the moment the fetch call returns: the
        // `[*.process]` step after it is long, silent and local — a CPU-bound
        // DEM crop produces no ticks and cancelling it would be a bug.
        let activity = activity.clone();
        let network_done = network_done.clone();
        let cancel = cancel.clone();
        let slot = slot.clone();
        let key = key.clone();
        bevy::tasks::AsyncComputeTaskPool::get()
            .spawn(async move {
                let mut last = u64::MAX; // sentinel: the first sample always counts as progress
                let mut idle = std::time::Duration::ZERO;
                loop {
                    std::thread::sleep(STALL_POLL);
                    if network_done.load(Ordering::Relaxed) || cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let now = activity.load(Ordering::Relaxed);
                    if now != last {
                        last = now;
                        idle = std::time::Duration::ZERO;
                        continue;
                    }
                    idle += STALL_POLL;
                    if idle < BODY_STALL_TIMEOUT {
                        continue;
                    }
                    // Give up. Raising the flag lets the fetch task unwind at
                    // its next chunk boundary if bytes ever resume; the state
                    // is published NOW regardless, because the whole point is
                    // that the user must not wait on a peer that is gone.
                    cancel.store(true, Ordering::Relaxed);
                    let secs = BODY_STALL_TIMEOUT.as_secs();
                    warn!("[datasets] '{key}' stalled — no data for {secs}s, giving up");
                    if let Ok(mut s) = slot.lock() {
                        *s = Some(DatasetState::Failed(format!(
                            "download stalled — no data for {secs}s (retry when the \
                             connection or the host recovers)"
                        )));
                    }
                    return;
                }
            })
            .detach();
    }

    let progress_activity = activity.clone();
    let extract_activity = activity;
    let fetch_done = network_done;
    bevy::tasks::AsyncComputeTaskPool::get()
        .spawn(async move {
            let control = DownloadControl {
                progress: Some(Box::new(move |done, total| {
                    progress_activity.fetch_add(1, Ordering::Relaxed);
                    if let Ok(mut s) = progress_slot.lock() {
                        *s = Some(DatasetState::Downloading {
                            bytes_done: done,
                            bytes_total: total,
                        });
                    }
                })),
                // Not for display — the tick is what tells the watchdog an
                // unpacking archive is alive.
                extracting: Some(Box::new(move |_entries| {
                    extract_activity.fetch_add(1, Ordering::Relaxed);
                })),
                cancel: Some(cancel),
            };
            // The scope decided the root: engine → shared cache, twin →
            // `<twin>/.cache`. Same resolver the CLI downloader uses, so a
            // file fetched from the app and one fetched from the terminal land
            // in exactly the same place.
            let fetched =
                download_asset_with_control(&spec, &key, control, Some(dest_root.as_path()));
            // Network phase over: retire the watchdog before the process step,
            // which is local, silent and legitimately slow.
            fetch_done.store(true, Ordering::Relaxed);
            let outcome = match fetched {
                // A download is only half of a derived dataset. The CLI has
                // always run `process` as a second command; in-app there is no
                // second command to run, so the fetch that a user authorised
                // has to produce the file they asked for — otherwise the UI
                // says "installed" and the consumer still finds nothing.
                Ok(()) => match run_process_step(&spec, &scope, &dest) {
                    Ok(()) => DatasetState::Installed,
                    Err(e) => DatasetState::Failed(format!("processing failed: {e}")),
                },
                Err(e) => DatasetState::Failed(e.to_string()),
            };
            if let Ok(mut s) = slot.lock() {
                *s = Some(outcome);
            }
        })
        .detach();
}

/// Run the entry's `[*.process]` step, if it declares one, into the same path
/// [`artifact_path`] reported — one resolver, so "installed" and "loadable"
/// cannot mean different files.
#[cfg(not(target_arch = "wasm32"))]
fn run_process_step(
    spec: &AssetEntry,
    scope: &DatasetScope,
    dest: &std::path::Path,
) -> Result<(), std::io::Error> {
    let Some(cfg) = &spec.process else {
        return Ok(());
    };
    let twin_root = match scope {
        DatasetScope::Twin { root, .. } => Some(root.clone()),
        DatasetScope::Engine => None,
    };
    info!("[datasets] processing '{}' ({})", cfg.kind, dest.display());
    crate::process::process_asset(dest, cfg, twin_root.as_deref())
}

/// The web build has no cache directory to fill and no HTTP downloader here;
/// its assets are served by the host. Requesting is a reported no-op rather
/// than a silent one.
#[cfg(target_arch = "wasm32")]
fn spawn_download(
    entry: &DatasetEntry,
    _spec: &AssetEntry,
    _scope: DatasetScope,
    slot: StatusSlot,
    _cancel: Arc<std::sync::atomic::AtomicBool>,
) {
    warn!(
        "[datasets] '{}' cannot be downloaded in the browser build — it is served by the host",
        entry.key
    );
    if let Ok(mut s) = slot.lock() {
        *s = Some(DatasetState::Failed("not downloadable on web".into()));
    }
}

/// Drain task-written status into the registry, and republish whatever failed
/// as [`DATASET_FAILED`] telemetry.
///
/// Cheap: one `try_lock` per dataset, and only while something is in flight.
/// The telemetry drain runs unconditionally — registration failures are raised
/// from `&mut self` methods that have no `Commands`, and they happen when
/// nothing is downloading.
fn drain_dataset_status(registry: Option<ResMut<DatasetRegistry>>, mut commands: Commands) {
    let Some(mut registry) = registry else { return };

    // Registration-time failures (parse, unreadable manifest, duplicate key).
    // Guarded so the common empty case never touches `ResMut`'s deref_mut and
    // marks the registry changed every frame.
    if !registry.pending_failures.is_empty() {
        for detail in std::mem::take(&mut registry.pending_failures) {
            commands.trigger(dataset_failed(detail));
        }
    }

    if !registry
        .entries
        .iter()
        .any(|e| matches!(e.state, DatasetState::Downloading { .. }))
    {
        return;
    }
    for i in 0..registry.entries.len() {
        let next = registry.slots[i]
            .status
            .lock()
            .ok()
            .and_then(|mut s| s.take());
        if let Some(state) = next {
            if let DatasetState::Failed(ref e) = state {
                warn!("[datasets] '{}' failed: {e}", registry.entries[i].key);
                // A failed download is a user-visible failure: the panel row
                // goes red, but the user may be looking anywhere else.
                commands.trigger(dataset_failed(format!(
                    "dataset '{}' failed: {e}",
                    registry.entries[i].key
                )));
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

/// Register every engine manifest in `assets/manifests/`, one group per file.
///
/// No crate declares anything in Rust: the manifests are data, read from the
/// shipped asset library at startup exactly as an open Twin's is read when it
/// opens. A crate that OWNS a dataset still owns what to do with the bytes —
/// it just no longer carries a compiled-in copy of the declaration, so adding a
/// dataset or fixing a URL is an edit to a `.toml`.
#[cfg(not(target_arch = "wasm32"))]
fn scan_engine_manifests(registry: Option<ResMut<DatasetRegistry>>) {
    let Some(mut registry) = registry else { return };
    let manifests = crate::engine_manifests();
    if manifests.is_empty() {
        // Not fatal — an app may ship no declarations at all — but it is also
        // exactly what a mis-staged package looks like, so say so once.
        info!(
            "[datasets] no manifests in {} — nothing is offered for download",
            crate::manifests_dir().display()
        );
        return;
    }
    let mut total = 0;
    for (group, path) in manifests {
        match std::fs::read_to_string(&path) {
            Ok(text) => total += registry.register(&text, &group),
            Err(e) => {
                error!("[datasets] cannot read {}: {e}", path.display());
                // Queued rather than triggered here: this is a `Startup`
                // system, and one path for every manifest failure means the
                // panel cannot learn about some of them and not others.
                registry
                    .pending_failures
                    .push(format!("cannot read {}: {e}", path.display()));
            }
        }
    }
    info!("[datasets] {total} declared dataset(s) from assets/manifests");
}

/// Adds the [`DatasetRegistry`], its status pump, the engine-manifest scan and
/// the open-Twin scan. Idempotent.
pub struct DatasetsPlugin;

impl Plugin for DatasetsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DatasetRegistry>();
        app.add_systems(Update, drain_dataset_status);
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Startup, scan_engine_manifests);
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

    /// A broken manifest must not be silent: the panel shows an empty list,
    /// which is indistinguishable from "nothing declared" unless the failure is
    /// published. `drain_dataset_status` turns each queued cause into a
    /// `DATASET_FAILED` Error telemetry event.
    #[test]
    fn a_broken_manifest_queues_a_user_visible_failure() {
        let mut r = DatasetRegistry::default();
        r.register("this is not toml {{{", "bad");
        assert_eq!(r.pending_failures.len(), 1);
        assert!(r.pending_failures[0].contains("bad"));

        // And a duplicate key is a failure too, not just a log line.
        let mut r = DatasetRegistry::default();
        r.register(MANIFEST, "first");
        r.register(MANIFEST, "second");
        assert_eq!(r.pending_failures.len(), 1);
        assert!(r.pending_failures[0].contains("demo_vectors"));

        let ev = dataset_failed("boom");
        assert_eq!(ev.name, DATASET_FAILED);
        assert_eq!(ev.severity, lunco_core::Severity::Error);
    }

    /// Cancelling something that is not in flight is a no-op, and cancelling a
    /// download releases the entry to `Failed` — which `request` accepts — so
    /// the user is never locked out by a wedged transfer.
    #[test]
    fn cancel_releases_the_entry_for_retry() {
        let mut r = DatasetRegistry::default();
        r.register(MANIFEST, "demo");
        r.cancel("demo_vectors"); // not downloading → no-op
        assert_eq!(r.entries()[0].state, DatasetState::Missing);

        // Simulate an in-flight download without touching the network.
        r.entries[0].state = DatasetState::Downloading {
            bytes_done: 0,
            bytes_total: 0,
        };
        r.cancel("demo_vectors");
        assert!(matches!(r.entries()[0].state, DatasetState::Failed(_)));
        // `missing()` — what the UI offers a download button for — includes it.
        assert_eq!(r.missing().count(), 1);
    }

    #[test]
    fn unknown_key_lookups_are_none() {
        let r = DatasetRegistry::default();
        assert!(r.state("nope").is_none());
        assert!(r.path("nope").is_none());
    }

    /// A derived dataset DELIVERS its process output, not its download. Calling
    /// it installed once the source lands would leave every consumer loading a
    /// file that does not exist yet.
    #[test]
    fn a_processed_dataset_is_identified_by_its_output_not_its_download() {
        const DERIVED: &str = r#"
[earthlike]
name = "Earthlike"
url = "https://example.invalid/source.jpg"
dest = "textures/earthlike_source.jpg"

[earthlike.process]
target_resolution = [4, 2]
output = "textures/earthlike.png"
"#;
        let mut r = DatasetRegistry::default();
        assert_eq!(r.register(DERIVED, "demo"), 1);
        let e = &r.entries()[0];
        assert_eq!(e.artifact_rel, "textures/earthlike.png");
        assert!(e.path.ends_with("textures/earthlike_source.jpg"));
        assert_eq!(e.artifact_uri(), "lunco://textures/earthlike.png");
    }

    /// A Twin's dataset addresses through `twin://`, so bytes that arrived
    /// inside the folder — an archive someone sent, `.cache` included — load
    /// through the same URI a freshly downloaded copy would.
    #[test]
    fn a_twin_dataset_addresses_through_its_own_scheme() {
        let mut r = DatasetRegistry::default();
        let scope = DatasetScope::Twin {
            name: "school".into(),
            root: PathBuf::from("/twins/school"),
        };
        assert_eq!(r.register_scoped(MANIFEST, "school", scope), 1);
        assert_eq!(
            r.entries()[0].artifact_uri(),
            "twin://school/ephemeris/demo.csv"
        );
    }

    /// Read roots are wider than the write root, and ordered: a copy packed
    /// into a distribution outranks the source-tree and machine-wide pools.
    #[test]
    fn engine_scope_reads_the_packed_cache_before_the_shared_pool() {
        let roots = DatasetScope::Engine.read_roots();
        assert_eq!(roots, crate::cache_roots());
        assert_eq!(roots[0], crate::packed_cache_dir());
        if let Some(development) = crate::development_cache_dir() {
            assert_eq!(roots[1], development);
        }
        let shared = crate::cache_dir();
        assert_eq!(roots.last(), Some(&shared));
        // The write root stays the shared pool: a package may be read-only,
        // and one machine should not hold a copy per installation.
        assert_eq!(DatasetScope::Engine.dest_root(), crate::cache_dir());
    }

    /// A Twin folder is self-contained in BOTH directions: its `.cache` is
    /// where downloads land and the first place reads look.
    #[test]
    fn twin_scope_reads_its_own_cache_then_its_authored_tree() {
        let root = PathBuf::from("/twins/school");
        let scope = DatasetScope::Twin {
            name: "school".into(),
            root: root.clone(),
        };
        assert_eq!(scope.dest_root(), crate::twin_cache_dir(&root));
        assert_eq!(scope.read_roots(), vec![crate::twin_cache_dir(&root), root]);
    }
}
