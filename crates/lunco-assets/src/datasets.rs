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
use lunco_core::{on_command, register_commands, Command};

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
    /// The download completed and the declared local processing step is
    /// running. The delivered artifact is not ready until this phase ends.
    Processing {
        /// Name of the manifest-declared processing pipeline.
        kind: String,
    },
    /// Cancellation was requested and the owned worker is unwinding. The row
    /// remains non-requestable until its task returns, so a retry can never
    /// race the previous attempt's cleanup or final write.
    Cancelling,
    /// The delivered artifact is complete at its declared destination.
    Installed,
    /// The owned operation completed after a user cancellation.
    Cancelled,
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
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DatasetScope {
    /// Declared by the engine (a crate's own `Assets.toml`) → the shared
    /// [`cache_dir`](crate::cache_dir).
    Engine,
    /// Declared by a Twin. The default write owner is that Twin's cache
    /// ([`twin_cache_dir`](crate::twin_cache_dir)); entries marked `shared`
    /// write to the global cache and every Twin can read that shared pool.
    Twin {
        /// `twin://` authority the root is registered under.
        name: String,
        /// Absolute Twin root.
        root: PathBuf,
    },
}

impl DatasetScope {
    /// Select the write owner for a Twin declaration. This is shared by the
    /// runtime registry and the standalone asset commands so `shared` cannot
    /// mean one location in the app and another on the CLI.
    pub fn twin_cache_root(root: &std::path::Path, shared: bool) -> PathBuf {
        if shared {
            crate::cache_dir()
        } else {
            crate::twin_cache_dir(root)
        }
    }

    /// The cache that owns a scoped entry's downloaded and derived artifacts.
    /// Engine datasets always use the global cache. A Twin opts into that same
    /// cache with `shared = true`; otherwise its artifacts stay beside it.
    pub fn cache_root(&self, shared: bool) -> PathBuf {
        match self {
            DatasetScope::Engine => crate::cache_dir(),
            DatasetScope::Twin { root, .. } => Self::twin_cache_root(root, shared),
        }
    }

    /// Every directory a scoped entry's file may be READ from, in priority
    /// order. Wider than [`cache_root`](Self::cache_root) on purpose: bytes that
    /// arrived with a distribution were never written by this machine.
    ///
    /// An engine dataset may ship inside the package (`assets/.cache`) rather
    /// than sitting in the machine's pool; a Twin's may arrive in the `.cache`
    /// of an archive someone sent. A Twin may also explicitly opt into the
    /// engine-wide shared cache. Both are installed — asking only where WE
    /// would have written would report them missing and offer to re-download a
    /// file already on disk. A Twin always has the global cache as its final
    /// read root: `shared = true` controls where that declaration writes,
    /// while logical Twin reads intentionally reuse an already-installed global
    /// copy.
    pub fn read_roots(&self) -> Vec<PathBuf> {
        match self {
            DatasetScope::Engine => {
                let mut roots = vec![crate::assets_dir_abs()];
                roots.extend(crate::cache_roots());
                roots
            }
            DatasetScope::Twin { root, .. } => {
                vec![
                    root.clone(),
                    crate::twin_cache_dir(root),
                    crate::cache_dir(),
                ]
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
    /// Globally unique registry identity. Manifest keys are only unique within
    /// one scope; callers must use this id for requests and queries.
    pub id: String,
    /// Manifest key (`[artemis2_vectors]` → `"artemis2_vectors"`), unique
    /// within its scope.
    pub key: String,
    /// Which registrant declared it — shown in UI groupings, e.g. `"ephemeris"`.
    pub group: String,
    /// Engine-declared or Twin-declared; decides the destination cache.
    pub scope: DatasetScope,
    /// Human-readable name from the manifest.
    pub name: String,
    /// Whether onboarding should offer this dataset when it is missing.
    pub recommended: bool,
    /// Where the file lands once downloaded.
    pub path: PathBuf,
    /// The artifact this dataset actually DELIVERS, relative to its scope root:
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
fn artifact_present(
    spec: &AssetEntry,
    path: &std::path::Path,
    source_path: Option<&std::path::Path>,
) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = spec;
        let _ = path;
        false
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(process) = &spec.process {
            crate::process::processed_output_present(path, process, source_path)
        } else {
            crate::download::installed_destination_present(spec, path)
        }
    }
}

impl DatasetEntry {
    /// Absolute path of the delivered artifact: the first
    /// [`read root`](DatasetScope::read_roots) that actually holds it, else the
    /// root a download would write it to.
    pub fn artifact_path(&self) -> PathBuf {
        let roots = self.scope.read_roots();
        for root in &roots {
            let candidate = root.join(&self.artifact_rel);
            if artifact_present(&self.spec, &candidate, None) {
                return candidate;
            }
        }
        self.scope
            .cache_root(self.spec.shared)
            .join(&self.artifact_rel)
    }

    fn source_path_for_root(&self, artifact_root: &std::path::Path) -> Option<PathBuf> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = artifact_root;
            None
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let source_rel = self.scope.read_roots().iter().find_map(|root| {
                self.path
                    .strip_prefix(root)
                    .ok()
                    .map(std::path::Path::to_path_buf)
            });
            let candidate = source_rel
                .map(|relative| artifact_root.join(relative))
                .filter(|path| path.is_file());
            candidate.or_else(|| self.path.is_file().then(|| self.path.clone()))
        }
    }

    /// The asset URI the delivered artifact loads at — `lunco://<rel>` for an
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
fn artifact_rel_of(
    entry: &AssetEntry,
    scope: &DatasetScope,
    dest: &std::path::Path,
) -> Result<String, std::io::Error> {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(cfg) = &entry.process {
        let twin_root = match scope {
            DatasetScope::Twin { root, .. } => Some(root.as_path()),
            DatasetScope::Engine => None,
        };
        let cache_root = scope.cache_root(entry.shared);
        let _ = crate::process::process_output_path(cfg, Some(&cache_root), twin_root)?;
        if cfg.output_root == "assets" {
            if matches!(scope, DatasetScope::Twin { .. }) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "process output_root=\"assets\" is engine-owned and cannot deliver a Twin artifact",
                ));
            }
            return Ok(cfg.output.clone());
        }
        if cfg.output_root == "twin" && !matches!(scope, DatasetScope::Twin { .. }) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "process output_root=\"twin\" requires a Twin-scoped dataset",
            ));
        }
        if cfg.output_root == "twin" {
            return Ok(cfg.output.clone());
        }
        let abs = crate::process::process_output_path(cfg, Some(&cache_root), twin_root)?;
        return abs
            .strip_prefix(&cache_root)
            .map(crate::asset_path::slashed)
            .map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "processed cache output {} is outside its owning cache {}",
                        abs.display(),
                        cache_root.display()
                    ),
                )
            });
    }
    let _ = entry;
    dest.strip_prefix(scope.cache_root(entry.shared))
        .map(crate::asset_path::slashed)
        .map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "download output {} is outside its owning cache for scope {}",
                    dest.display(),
                    scope.label()
                ),
            )
        })
}

/// Cross-thread slot a download task writes its progress into.
type StatusSlot = Arc<Mutex<Option<DatasetState>>>;

/// Telemetry event name published when a declared dataset cannot be offered or
/// cannot be fetched: an unparseable `Assets.toml`, a manifest file that will
/// not read, a duplicate key, or a failed download.
///
/// Published at [`Severity::Error`](lunco_core::Severity::Error) so the status
/// bar's error-telemetry observer surfaces it. A user whose dataset panel is
/// empty because a manifest is broken must
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

/// Everything one download ATTEMPT communicates with. The registry owns the
/// task for the full operation lifetime; no worker is detached or allowed to
/// outlive the slot that represents it.
struct DownloadHandle {
    status: StatusSlot,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    commit_gate: Arc<Mutex<()>>,
    #[cfg(not(target_arch = "wasm32"))]
    task: Option<bevy::tasks::Task<DatasetState>>,
}

impl DownloadHandle {
    fn new(commit_gate: Arc<Mutex<()>>) -> Self {
        Self {
            status: Arc::new(Mutex::new(None)),
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            commit_gate,
            #[cfg(not(target_arch = "wasm32"))]
            task: None,
        }
    }
}

/// Every dataset any crate has declared, and its live state.
///
/// Registration order is irrelevant; the derived dataset id is unique, and a
/// duplicate declaration is refused rather than silently overwriting another
/// dataset. The same key may legitimately occur in different manifest groups
/// because the group is part of the engine-scoped id.
#[derive(Resource, Default)]
pub struct DatasetRegistry {
    entries: Vec<DatasetEntry>,
    /// Per-entry download handle, written by the task, drained in `Update`.
    slots: Vec<DownloadHandle>,
    /// Twin scopes already scanned, including Twins with no `Assets.toml`.
    /// This is lifecycle state, not inferred from entry count: an empty Twin
    /// still has to be remembered or it is rescanned every frame.
    scanned_scopes: Vec<DatasetScope>,
    /// Failures raised from `&mut self` methods, which have no `Commands`.
    /// Drained into [`DATASET_FAILED`] telemetry by [`drain_dataset_status`].
    pending_failures: Vec<String>,
    /// Operations removed from the visible registry by Twin close. They stay
    /// owned here until cancellation and staging cleanup have completed.
    #[cfg(not(target_arch = "wasm32"))]
    retiring: Vec<DownloadHandle>,
    /// Shared write barrier for all dataset attempts. Twin close acquires this
    /// before retiring an attempt, so no download or processing commit can
    /// happen after the close boundary returns.
    commit_gate: Arc<Mutex<()>>,
}

/// User intent to start a declared dataset download.
///
/// The UI emits this event instead of mutating [`DatasetRegistry`] directly;
/// the registry remains the only owner of download authorisation and task
/// lifecycle.
#[Command]
pub struct RequestDataset {
    /// Globally unique dataset id from [`DatasetEntry::id`].
    pub id: String,
}

/// Cancel a declared dataset download. The operation remains owned until its
/// worker returns, after which the row becomes requestable again.
#[Command]
pub struct CancelDataset {
    /// Globally unique dataset id from [`DatasetEntry::id`].
    pub id: String,
}

/// A dataset scope has been scanned and its declared datasets are now stable
/// for this lifecycle revision. Consumers can make one decision from this
/// event; they must not poll the registry to infer whether the scan already
/// happened.
#[derive(Event, Clone, Debug)]
pub struct DatasetScopeReady {
    /// The engine or Twin whose manifest was scanned.
    pub scope: DatasetScope,
}

/// A Twin scope has been removed. In-flight tasks for it have already been
/// cancelled by the registry before this event is emitted.
#[derive(Event, Clone, Debug)]
pub struct DatasetScopeRemoved {
    /// The removed Twin scope.
    pub scope: DatasetScope,
}

/// A dataset's delivered artifact became ready after its download and local
/// processing completed.
///
/// The event carries the consumer-facing URI, not the temporary download path.
/// Consumers that already asked Bevy for this asset can therefore refresh only
/// this artifact; consumers that had not asked yet simply load it normally.
#[derive(Event, Clone, Debug)]
pub struct DatasetInstalled {
    /// Globally unique dataset id.
    pub id: String,
    /// Scope that owns the installed bytes.
    pub scope: DatasetScope,
    /// Delivered artifact on disk, including a processed output when declared.
    pub artifact_path: PathBuf,
    /// Asset-server URI for the delivered artifact.
    pub artifact_uri: String,
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
        let mut added = 0;
        for (key, entry) in manifest.assets {
            // The complete id includes scope, group, and key. Two Twins may
            // both declare `dtm`, and engine groups may reuse a key without
            // shadowing one another.
            let id = dataset_id(&scope, group, &key);
            if self.entries.iter().any(|e| e.id == id) {
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
            let dest_root = scope.cache_root(entry.shared);
            let path = match entry_dest_path(&entry, Some(&dest_root)) {
                Ok(path) => path,
                Err(error) => {
                    error!(
                        "[datasets] invalid destination for '{key}' in scope '{}': {error}",
                        scope.label()
                    );
                    self.pending_failures.push(format!(
                        "dataset '{key}' in scope '{}' has an invalid destination: {error}",
                        scope.label()
                    ));
                    continue;
                }
            };
            let artifact_rel = match artifact_rel_of(&entry, &scope, &path) {
                Ok(relative) => relative,
                Err(error) => {
                    error!(
                        "[datasets] invalid processed output for '{key}' in scope '{}': {error}",
                        scope.label()
                    );
                    self.pending_failures.push(format!(
                        "dataset '{key}' in scope '{}' has an invalid processed output: {error}",
                        scope.label()
                    ));
                    continue;
                }
            };
            let installed = scope.read_roots().iter().any(|r| {
                let artifact = r.join(&artifact_rel);
                artifact_present(&entry, &artifact, None)
            });
            self.entries.push(DatasetEntry {
                id,
                key: key.clone(),
                group: group.to_string(),
                scope: scope.clone(),
                name: entry.name.clone(),
                recommended: entry.recommended,
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
            self.slots
                .push(DownloadHandle::new(self.commit_gate.clone()));
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
        self.scanned_scopes.push(scope.clone());
        let manifest_path = root.join("Assets.toml");
        let text = match std::fs::read_to_string(&manifest_path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return 0; // A Twin without a manifest declares no datasets.
            }
            Err(error) => {
                self.pending_failures.push(format!(
                    "cannot read Twin manifest {}: {error}",
                    manifest_path.display()
                ));
                return 0;
            }
        };
        let n = self.register_scoped(&text, name, scope);
        if n > 0 {
            info!("[datasets] twin '{name}': {n} declared dataset(s)");
        }
        n
    }

    /// Drop every entry of a scope (a Twin closing, or a rescan). In-flight
    /// operations are moved to the retiring queue and remain owned until their
    /// cancellation and staging cleanup complete.
    pub fn forget_scope(&mut self, scope: &DatasetScope) {
        let gate = self.commit_gate.clone();
        let _gate_locked = match gate.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                self.pending_failures.push(format!(
                    "dataset commit gate was poisoned while retiring scope '{}'",
                    scope.label()
                ));
                poisoned.into_inner()
            }
        };
        let mut i = 0;
        while i < self.entries.len() {
            if &self.entries[i].scope == scope {
                self.slots[i]
                    .cancel
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                self.entries.remove(i);
                #[cfg(not(target_arch = "wasm32"))]
                self.retiring.push(self.slots.remove(i));
                #[cfg(target_arch = "wasm32")]
                self.slots.remove(i);
            } else {
                i += 1;
            }
        }
        self.scanned_scopes.retain(|known| known != scope);
    }

    /// Drop every Twin-scoped registry state backed by `root` and return the
    /// removed scopes for lifecycle observers.
    ///
    /// Twin close is an authoritative event, not a filesystem observation. The
    /// registry must therefore retire the scope immediately, including an
    /// in-flight download, even when the replacement Twin is registered before
    /// the next update scan runs.
    fn forget_twin_root(&mut self, root: &std::path::Path) -> Vec<DatasetScope> {
        let mut scopes = Vec::new();
        for scope in self
            .entries
            .iter()
            .map(|entry| &entry.scope)
            .chain(self.scanned_scopes.iter())
        {
            if matches!(scope, DatasetScope::Twin { root: scope_root, .. } if scope_root == root)
                && !scopes.contains(scope)
            {
                scopes.push(scope.clone());
            }
        }
        for scope in &scopes {
            self.forget_scope(scope);
        }
        scopes
    }

    /// Re-read on-disk presence for every entry. Cheap (`Path::exists` per
    /// dataset) and only meaningful for entries without an owned operation.
    pub fn refresh_installed_state(&mut self) {
        for e in &mut self.entries {
            if matches!(
                e.state,
                DatasetState::Downloading { .. }
                    | DatasetState::Processing { .. }
                    | DatasetState::Cancelling
            ) {
                continue;
            }
            let installed = e.scope.read_roots().iter().any(|root| {
                let artifact = root.join(&e.artifact_rel);
                artifact_present(&e.spec, &artifact, e.source_path_for_root(root).as_deref())
            });
            e.state = if installed {
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

    /// Twin scopes whose manifests have been scanned, including empty ones.
    pub fn scanned_scopes(&self) -> &[DatasetScope] {
        &self.scanned_scopes
    }

    /// Whether a scope has completed its manifest scan for this lifecycle.
    ///
    /// Consumers that project a declared resource must wait for this boundary:
    /// an empty manifest is still a valid answer, while consulting the registry
    /// before the scan would confuse "not discovered yet" with "not declared".
    pub fn is_scope_scanned(&self, scope: &DatasetScope) -> bool {
        self.scanned_scopes.iter().any(|scanned| scanned == scope)
    }

    /// Find the declaration that delivers one scope-relative artifact.
    ///
    /// The artifact path, rather than a domain-specific dataset key, is the
    /// contract shared with consumers. This lets a USD source wait for a
    /// processed product without learning the manifest's group or inventing a
    /// second terrain-resource identity.
    pub fn declared_artifact(
        &self,
        scope: &DatasetScope,
        relative: &std::path::Path,
    ) -> Option<&DatasetEntry> {
        let relative = crate::asset_path::slashed(relative)
            .trim_start_matches('/')
            .to_owned();
        self.entries
            .iter()
            .find(|entry| &entry.scope == scope && entry.artifact_rel == relative)
    }

    /// State of one globally identified dataset.
    pub fn state(&self, id: &str) -> Option<&DatasetState> {
        self.entries.iter().find(|e| e.id == id).map(|e| &e.state)
    }

    /// On-disk path of the artifact one dataset DELIVERS (its `[*.process]` output
    /// where it has one), or `None` if nothing declared that key. This is the
    /// path a consumer loads; [`DatasetEntry::path`] is where the download
    /// landed, which for a derived product is not the same file.
    pub fn path(&self, id: &str) -> Option<PathBuf> {
        self.entries
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.artifact_path())
    }

    /// The installed dataset delivering `key`, or `None` when it is not
    /// declared or not on disk. The one call a consumer needs: "are these bytes
    /// available, and where?".
    pub fn installed(&self, id: &str) -> Option<&DatasetEntry> {
        self.entries
            .iter()
            .find(|e| e.id == id && e.state.is_installed())
    }

    /// Datasets that are declared but not on disk.
    pub fn missing(&self) -> impl Iterator<Item = &DatasetEntry> {
        self.entries.iter().filter(|e| {
            matches!(
                e.state,
                DatasetState::Missing | DatasetState::Failed(_) | DatasetState::Cancelled
            )
        })
    }

    /// Start downloading `id`. **The only call in the engine that authorises
    /// network traffic for declared assets** — wire it to an explicit user
    /// action, never to startup or scene load.
    ///
    /// No-op when the dataset is already installed or already downloading.
    ///
    /// An attempt remains owned until its task returns. A retry can therefore
    /// never overlap staging or installation from an earlier attempt.
    pub fn request(&mut self, id: &str) {
        let Some(i) = self.entries.iter().position(|e| e.id == id) else {
            warn!("[datasets] request for unknown dataset '{id}'");
            return;
        };
        if matches!(
            self.entries[i].state,
            DatasetState::Installed
                | DatasetState::Downloading { .. }
                | DatasetState::Processing { .. }
                | DatasetState::Cancelling
        ) {
            return;
        }
        self.entries[i].state = DatasetState::Downloading {
            bytes_done: 0,
            bytes_total: 0,
        };
        // Fresh handle per attempt — see `DownloadHandle`. A retry after a
        // stall must not be able to inherit the abandoned task's verdict.
        self.slots[i] = DownloadHandle::new(self.commit_gate.clone());
        let scope = self.entries[i].scope.clone();
        let spec = self.entries[i].spec.clone();
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.slots[i].task = Some(spawn_download(
                &self.entries[i],
                &spec,
                scope,
                self.slots[i].status.clone(),
                self.slots[i].cancel.clone(),
                self.slots[i].commit_gate.clone(),
            ));
        }
        #[cfg(target_arch = "wasm32")]
        spawn_download(
            &self.entries[i],
            &spec,
            scope,
            self.slots[i].status.clone(),
            self.slots[i].cancel.clone(),
            self.slots[i].commit_gate.clone(),
        );
    }

    /// Request cooperative cancellation. The operation remains owned by the
    /// registry and the row becomes retryable only after its task returns.
    pub fn cancel(&mut self, id: &str) {
        let Some(i) = self.entries.iter().position(|e| e.id == id) else {
            warn!("[datasets] cancel for unknown dataset '{id}'");
            return;
        };
        if !matches!(
            self.entries[i].state,
            DatasetState::Downloading { .. } | DatasetState::Processing { .. }
        ) {
            return;
        }
        self.slots[i]
            .cancel
            .store(true, std::sync::atomic::Ordering::Release);
        self.entries[i].state = DatasetState::Cancelling;
        info!("[datasets] '{id}' download cancelled by user");
    }

    /// Start every missing dataset. Same authorisation rule as [`request`](Self::request).
    pub fn request_all_missing(&mut self) {
        let ids: Vec<String> = self.missing().map(|e| e.id.clone()).collect();
        for id in ids {
            self.request(&id);
        }
    }
}

pub fn dataset_id(scope: &DatasetScope, group: &str, key: &str) -> String {
    match scope {
        DatasetScope::Engine => format!("engine/{group}/{key}"),
        DatasetScope::Twin { name, .. } => format!("twin/{name}/{key}"),
    }
}

#[on_command(RequestDataset)]
fn on_request_dataset(trigger: On<RequestDataset>, mut registry: ResMut<DatasetRegistry>) {
    registry.request(&trigger.event().id);
}

#[on_command(CancelDataset)]
fn on_cancel_dataset(trigger: On<CancelDataset>, mut registry: ResMut<DatasetRegistry>) {
    registry.cancel(&trigger.event().id);
}

/// Retire Twin-scoped dataset state at the same lifecycle edge that retires the
/// Twin asset root. The update scanner remains responsible for discovering new
/// roots, but close must not wait for that scanner: a close followed immediately
/// by a same-name reopen must still get a fresh manifest scan.
fn on_twin_closed(
    trigger: On<lunco_workspace::TwinClosed>,
    mut registry: ResMut<DatasetRegistry>,
    mut commands: Commands,
) {
    for scope in registry.forget_twin_root(&trigger.event().root) {
        commands.trigger(DatasetScopeRemoved { scope });
    }
}

/// Spawn the complete fetch and processing operation as one registry-owned task.
/// Transport-level receive deadlines remain in the downloader; operation
/// lifetime is represented by the owned task rather than a second watchdog.
#[cfg(not(target_arch = "wasm32"))]
fn spawn_download(
    entry: &DatasetEntry,
    spec: &AssetEntry,
    scope: DatasetScope,
    slot: StatusSlot,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    commit_gate: Arc<Mutex<()>>,
) -> bevy::tasks::Task<DatasetState> {
    use crate::download::{download_asset_with_control, DownloadControl};
    use std::sync::atomic::Ordering;

    let key = entry.key.clone();
    let name = entry.name.clone();
    let spec = spec.clone();
    let dest = entry.path.clone();
    let dest_root = scope.cache_root(spec.shared);
    let progress_slot = slot.clone();

    info!("[datasets] downloading '{key}' ({name}) — user-requested");
    bevy::tasks::AsyncComputeTaskPool::get().spawn(async move {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let process_control =
                crate::process::ProcessControl::new(cancel.clone(), commit_gate.clone());
            let download_cancel = cancel.clone();
            let extracting_slot = slot.clone();
            let download_control = DownloadControl {
                progress: Some(Box::new(move |done, total| {
                    if let Ok(mut s) = progress_slot.lock() {
                        *s = Some(DatasetState::Downloading {
                            bytes_done: done,
                            bytes_total: total,
                        });
                    }
                })),
                extracting: Some(Box::new(move |entries| {
                    if let Ok(mut s) = extracting_slot.lock() {
                        *s = Some(DatasetState::Processing {
                            kind: format!("extracting archive ({entries} entries)"),
                        });
                    }
                })),
                cancel: Some(download_cancel),
                commit_gate: Some(commit_gate.clone()),
            };
            // The scope decided the root: engine → shared cache, twin →
            // `<twin>/.cache`. Same resolver the CLI downloader uses, so a
            // file fetched from the app and one fetched from the terminal land
            // in exactly the same place.
            let fetched = download_asset_with_control(
                &spec,
                &key,
                download_control,
                Some(dest_root.as_path()),
            );
            match fetched {
                // A download is only half of a derived dataset. The CLI has
                // always run `process` as a second command; in-app there is no
                // second command to run, so the fetch that a user authorised
                // has to produce the file they asked for — otherwise the UI
                // says "installed" and the consumer still finds nothing.
                Ok(()) => {
                    if let Some(process) = &spec.process {
                        if let Ok(mut s) = slot.lock() {
                            *s = Some(DatasetState::Processing {
                                kind: process.kind.clone(),
                            });
                        }
                    }
                    match run_process_step(&spec, &scope, &dest, &process_control) {
                        Ok(()) => DatasetState::Installed,
                        Err(e) if cancel.load(Ordering::Acquire) => {
                            let _ = e;
                            DatasetState::Cancelled
                        }
                        Err(e) => DatasetState::Failed(format!("processing failed: {e}")),
                    }
                }
                Err(crate::download::DownloadError::Cancelled) => DatasetState::Cancelled,
                Err(e) => DatasetState::Failed(e.to_string()),
            }
        }))
        .unwrap_or_else(|_| DatasetState::Failed("dataset worker panicked".into()));
        #[cfg(target_arch = "wasm32")]
        if let Ok(mut s) = slot.lock() {
            *s = Some(outcome.clone());
        }
        outcome
    })
}

/// Run the entry's `[*.process]` step, if it declares one, into the same path
/// [`artifact_path`] reported — one resolver, so "installed" and "loadable"
/// cannot mean different files.
#[cfg(not(target_arch = "wasm32"))]
fn run_process_step(
    spec: &AssetEntry,
    scope: &DatasetScope,
    dest: &std::path::Path,
    control: &crate::process::ProcessControl,
) -> Result<(), std::io::Error> {
    let Some(cfg) = &spec.process else {
        return Ok(());
    };
    let twin_root = match scope {
        DatasetScope::Twin { root, .. } => Some(root.clone()),
        DatasetScope::Engine => None,
    };
    let cache_root = scope.cache_root(spec.shared);
    info!("[datasets] processing '{}' ({})", cfg.kind, dest.display());
    crate::process::process_asset(dest, cfg, &cache_root, twin_root.as_deref(), control)
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
    _commit_gate: Arc<Mutex<()>>,
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

    #[cfg(not(target_arch = "wasm32"))]
    {
        use bevy::tasks::futures_lite::future;
        let mut retiring = Vec::new();
        for mut handle in std::mem::take(&mut registry.retiring) {
            let finished = handle
                .task
                .as_mut()
                .and_then(|task| future::block_on(future::poll_once(task)))
                .is_some();
            if !finished {
                retiring.push(handle);
            }
        }
        registry.retiring = retiring;
    }
    #[cfg(not(target_arch = "wasm32"))]
    let has_live_work = registry
        .entries
        .iter()
        .zip(&registry.slots)
        .any(|(_, slot)| slot.task.is_some())
        || !registry.retiring.is_empty();
    #[cfg(target_arch = "wasm32")]
    let has_live_work = registry.entries.iter().any(|entry| {
        matches!(
            entry.state,
            DatasetState::Downloading { .. }
                | DatasetState::Processing { .. }
                | DatasetState::Cancelling
        )
    });
    if !has_live_work {
        return;
    }
    for i in 0..registry.entries.len() {
        let status_slot = registry.slots[i].status.clone();
        let next = match status_slot.lock() {
            Ok(mut status) => status.take(),
            Err(_) => {
                let detail = format!(
                    "dataset '{}' status channel is poisoned",
                    registry.entries[i].key
                );
                registry.pending_failures.push(detail.clone());
                Some(DatasetState::Failed(detail))
            }
        };
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
                let entry = &registry.entries[i];
                commands.trigger(DatasetInstalled {
                    id: entry.id.clone(),
                    scope: entry.scope.clone(),
                    artifact_path: entry.artifact_path(),
                    artifact_uri: entry.artifact_uri(),
                });
            }
            registry.entries[i].state = state;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            use bevy::tasks::futures_lite::future;
            let completed = registry.slots[i]
                .task
                .as_mut()
                .and_then(|task| future::block_on(future::poll_once(task)));
            if let Some(state) = completed {
                registry.slots[i].task = None;
                if let DatasetState::Failed(ref error) = state {
                    commands.trigger(dataset_failed(format!(
                        "dataset '{}' failed: {error}",
                        registry.entries[i].key
                    )));
                }
                if state.is_installed() {
                    let entry = &registry.entries[i];
                    commands.trigger(DatasetInstalled {
                        id: entry.id.clone(),
                        scope: entry.scope.clone(),
                        artifact_path: entry.artifact_path(),
                        artifact_uri: entry.artifact_uri(),
                    });
                }
                registry.entries[i].state = state;
            }
        }
    }
}

/// Refresh a delivered asset only after its complete producer pipeline has
/// finished. This handles consumers that requested a missing URI before the
/// download began: the same logical URI is re-read from the newly available
/// cache artifact, without reloading unrelated assets or rebuilding a scene.
fn reload_installed_asset(trigger: On<DatasetInstalled>, asset_server: Option<Res<AssetServer>>) {
    let Some(asset_server) = asset_server else {
        return;
    };
    asset_server.reload(trigger.event().artifact_uri.clone());
}

/// Discover Twin manifests for newly mounted roots. Closing is handled by the
/// authoritative [`lunco_workspace::TwinClosed`] observer above; the scan is
/// deliberately not responsible for teardown because a root can close and be
/// replaced before the next update frame.
///
/// [`TwinRoots`](crate::TwinRoots) is mutated through interior mutability (no
/// Bevy change detection), so discovery still diffs the name set. That is a
/// lock plus a small `Vec<String>` per frame, against a registry that is at most
/// a handful of Twins.
#[cfg(not(target_arch = "wasm32"))]
fn scan_open_twins_for_datasets(
    roots: Option<Res<crate::TwinRoots>>,
    registry: Option<ResMut<DatasetRegistry>>,
    mut commands: Commands,
) {
    let (Some(roots), Some(mut registry)) = (roots, registry) else {
        return;
    };
    let open = match roots.names() {
        Ok(open) => open,
        Err(error) => {
            lunco_core::trigger_error(
                &mut commands,
                "twin-dataset-registry-unavailable",
                format!("could not enumerate open Twins for dataset discovery: {error}"),
            );
            return;
        }
    };

    // New: scan any open Twin the registry has not seen. The tracked scope,
    // rather than entry count, handles a valid Twin with no manifest.
    for name in open {
        match roots.root_for(&name) {
            Ok(Some(root)) => {
                let scope = DatasetScope::Twin {
                    name: name.clone(),
                    root: root.clone(),
                };
                if registry.scanned_scopes.contains(&scope) {
                    continue;
                }
                registry.scan_twin(&name, &root);
                commands.trigger(DatasetScopeReady { scope });
            }
            Ok(None) => {}
            Err(error) => {
                lunco_core::trigger_error(
                    &mut commands,
                    "twin-dataset-registry-unavailable",
                    format!("could not resolve Twin `{name}` for dataset discovery: {error}"),
                );
                return;
            }
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
fn scan_engine_manifests(registry: Option<ResMut<DatasetRegistry>>, mut commands: Commands) {
    let Some(mut registry) = registry else { return };
    let manifests = match crate::engine_manifests() {
        Ok(manifests) => manifests,
        Err(error) => {
            registry.pending_failures.push(format!(
                "cannot enumerate engine manifests in {}: {error}",
                crate::manifests_dir().display()
            ));
            commands.trigger(DatasetScopeReady {
                scope: DatasetScope::Engine,
            });
            return;
        }
    };
    if manifests.is_empty() {
        // Not fatal — an app may ship no declarations at all — but it is also
        // exactly what a mis-staged package looks like, so say so once.
        info!(
            "[datasets] no manifests in {} — nothing is offered for download",
            crate::manifests_dir().display()
        );
        commands.trigger(DatasetScopeReady {
            scope: DatasetScope::Engine,
        });
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
    commands.trigger(DatasetScopeReady {
        scope: DatasetScope::Engine,
    });
}

/// Adds the [`DatasetRegistry`], its status pump, the engine-manifest scan and
/// the open-Twin scan. Idempotent.
pub struct DatasetsPlugin;

impl Plugin for DatasetsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DatasetRegistry>();
        register_commands!(on_request_dataset, on_cancel_dataset);
        register_all_commands(app);
        app.add_observer(reload_installed_asset);
        app.add_observer(on_twin_closed);
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
    use std::path::Path;

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
    fn duplicate_ids_are_refused_not_overwritten() {
        let mut r = DatasetRegistry::default();
        assert_eq!(r.register(MANIFEST, "first"), 1);
        assert_eq!(r.register(MANIFEST, "first"), 0);
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
        r.register(MANIFEST, "first");
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
        let id = r.entries()[0].id.clone();
        r.cancel(&id); // not downloading → no-op
        assert_eq!(r.entries()[0].state, DatasetState::Missing);

        // Simulate an in-flight download without touching the network.
        r.entries[0].state = DatasetState::Downloading {
            bytes_done: 0,
            bytes_total: 0,
        };
        r.cancel(&id);
        assert_eq!(r.entries()[0].state, DatasetState::Cancelling);
        // While cleanup is in progress the row is not requestable; retry is
        // exposed only after the owned task returns.
        assert_eq!(r.missing().count(), 0);
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
kind = "texture"
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

    #[test]
    fn a_processed_directory_requires_its_completion_stamp() {
        const DERIVED: &str = r#"
[luna2]
name = "Luna 2 terrain"
url = "https://example.invalid/luna2.tif"
dest = "terrain/luna2/source.tif"

[luna2.process]
kind = "dem"
output = "terrain/luna2"
"#;
        let twin = tempfile::tempdir().expect("temporary Twin root");
        let scope = DatasetScope::Twin {
            name: "luna2".into(),
            root: twin.path().to_path_buf(),
        };
        let output = crate::twin_cache_dir(twin.path()).join("terrain/luna2");
        std::fs::create_dir_all(&output).expect("partial processed directory");

        let mut registry = DatasetRegistry::default();
        assert_eq!(registry.register_scoped(DERIVED, "luna2", scope), 1);
        assert_eq!(registry.entries()[0].state, DatasetState::Missing);

        std::fs::write(output.join(".bakekey"), "complete").expect("completion stamp");
        std::fs::create_dir_all(output.join("materials/textures")).expect("terrain payload");
        std::fs::write(
            output.join("materials/textures/heightmap.tif"),
            b"heightmap",
        )
        .expect("heightmap payload");
        registry.refresh_installed_state();
        assert_eq!(registry.entries()[0].state, DatasetState::Installed);
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

    #[test]
    fn declared_artifact_matches_the_scope_relative_delivery_path() {
        let mut r = DatasetRegistry::default();
        let scope = DatasetScope::Twin {
            name: "school".into(),
            root: PathBuf::from("/twins/school"),
        };
        assert_eq!(r.register_scoped(MANIFEST, "school", scope.clone()), 1);
        assert_eq!(
            r.declared_artifact(&scope, Path::new("ephemeris/demo.csv"))
                .map(|entry| entry.key.as_str()),
            Some("demo_vectors")
        );
        assert!(r
            .declared_artifact(&scope, Path::new("terrain/missing"))
            .is_none());
    }

    /// Read roots are wider than the write root, and ordered: a copy packed
    /// into a distribution outranks the source-tree and machine-wide pools.
    #[test]
    fn engine_scope_reads_the_packed_cache_before_the_shared_pool() {
        let roots = DatasetScope::Engine.read_roots();
        assert_eq!(roots[0], crate::assets_dir_abs());
        assert_eq!(roots[1], crate::packed_cache_dir());
        if let Some(development) = crate::development_cache_dir() {
            assert_eq!(roots[2], development);
        }
        let shared = crate::cache_dir();
        assert_eq!(roots.last(), Some(&shared));
        // The write root stays the shared pool: a package may be read-only,
        // and one machine should not hold a copy per installation.
        assert_eq!(DatasetScope::Engine.cache_root(false), crate::cache_dir());
    }

    /// A Twin folder writes to its own `.cache`; authored files remain first
    /// priority, and an explicitly shared dataset is read from the engine pool.
    #[test]
    fn twin_scope_reads_its_own_cache_then_its_authored_tree() {
        let root = PathBuf::from("/twins/school");
        let scope = DatasetScope::Twin {
            name: "school".into(),
            root: root.clone(),
        };
        assert_eq!(scope.cache_root(false), crate::twin_cache_dir(&root));
        assert_eq!(scope.cache_root(true), crate::cache_dir());
        assert_eq!(
            scope.read_roots(),
            vec![
                root.clone(),
                crate::twin_cache_dir(&root),
                crate::cache_dir()
            ]
        );
    }

    #[test]
    fn closing_a_twin_root_retires_its_registry_scope_immediately() {
        let root = PathBuf::from("/twins/school");
        let scope = DatasetScope::Twin {
            name: "school".into(),
            root: root.clone(),
        };
        let mut registry = DatasetRegistry::default();
        assert_eq!(
            registry.register_scoped(MANIFEST, "school", scope.clone()),
            1
        );
        registry.scanned_scopes.push(scope.clone());

        let removed = registry.forget_twin_root(&root);

        assert_eq!(removed, vec![scope]);
        assert!(registry.entries().is_empty());
        assert!(registry.scanned_scopes().is_empty());
    }

    #[test]
    fn closing_an_empty_twin_root_retires_its_scanned_scope() {
        let root = PathBuf::from("/twins/empty");
        let scope = DatasetScope::Twin {
            name: "empty".into(),
            root: root.clone(),
        };
        let mut registry = DatasetRegistry::default();
        registry.scanned_scopes.push(scope.clone());

        let removed = registry.forget_twin_root(&root);

        assert_eq!(removed, vec![scope]);
        assert!(registry.scanned_scopes().is_empty());
    }
}
