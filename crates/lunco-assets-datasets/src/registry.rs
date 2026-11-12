//! Shared dataset identity, state, and Bevy-facing lifecycle events.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use lunco_core::Command;

use crate::{AssetEntry, AssetManifest, entry_dest_path, process_output_path};
#[cfg(not(target_arch = "wasm32"))]
use crate::{installed_destination_present, processed_output_present};

/// What a declared dataset is currently doing.
#[derive(Debug, Clone, PartialEq)]
pub enum DatasetState {
    /// Declared, not on disk.
    Missing,
    /// Explicitly requested and waiting for a bounded worker slot.
    Queued,
    /// A user-requested download is running.
    Downloading {
        /// Human-readable file currently being transferred.
        source: String,
        /// Bytes received so far.
        bytes_done: u64,
        /// Expected bytes, or zero when unknown.
        bytes_total: u64,
    },
    /// A manifest-declared local processing pipeline is running.
    Processing {
        /// Manifest-declared processing pipeline.
        kind: String,
    },
    /// Cancellation was requested and the worker is unwinding.
    Cancelling,
    /// The delivered artifact is complete.
    Installed,
    /// The owned operation completed after cancellation.
    Cancelled,
    /// The last operation failed.
    Failed(String),
}

impl DatasetState {
    /// Stable lifecycle phase shared by queries, policies, and diagnostics.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Queued => "queued",
            Self::Downloading { .. } => "downloading",
            Self::Processing { .. } => "processing",
            Self::Cancelling => "cancelling",
            Self::Installed => "installed",
            Self::Cancelled => "cancelled",
            Self::Failed(_) => "failed",
        }
    }

    /// Whether the delivered artifact is available.
    pub fn is_installed(&self) -> bool {
        matches!(self, Self::Installed)
    }
}

/// Scope that owns a dataset declaration and its cache.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DatasetScope {
    /// An engine manifest using the shared cache.
    Engine,
    /// A Twin manifest and its absolute root.
    Twin {
        /// Stable source identity used for dataset IDs across mount lifetimes.
        source_identity: String,
        /// Live `twin://` authority. Offline verification has no runtime address.
        authority: Option<String>,
        /// Human-readable manifest or folder label.
        display_name: String,
        /// Absolute Twin root.
        root: PathBuf,
    },
}

impl DatasetScope {
    /// Transport aliases of the same admitted canonical root share one store.
    pub fn same_storage(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Engine, Self::Engine) => true,
            (Self::Twin { root: left, .. }, Self::Twin { root: right, .. }) => left == right,
            _ => false,
        }
    }
    /// Stable command/query identity, distinct from the mount and display names.
    pub fn id(&self) -> String {
        match self {
            Self::Engine => "engine".to_owned(),
            Self::Twin {
                source_identity, ..
            } => format!("twin:{source_identity}"),
        }
    }

    /// Resolve the cache that owns a Twin's download.
    pub fn twin_cache_root(root: &Path, shared: bool) -> PathBuf {
        if shared {
            lunco_assets_core::cache_dir()
        } else {
            lunco_assets_core::twin_cache_dir(root)
        }
    }

    /// Cache that owns the downloaded and derived artifacts.
    pub fn cache_root(&self, shared: bool) -> PathBuf {
        match self {
            Self::Engine => lunco_assets_core::cache_dir(),
            Self::Twin { root, .. } => Self::twin_cache_root(root, shared),
        }
    }

    /// Roots searched for a delivered artifact, in precedence order.
    pub fn read_roots(&self) -> std::io::Result<Vec<PathBuf>> {
        Ok(match self {
            Self::Engine => {
                lunco_assets_core::asset_read_roots(&lunco_assets_core::assets_dir_abs()?)
            }
            Self::Twin { root, .. } => lunco_assets_core::asset_read_roots(root),
        })
    }

    /// Human-readable grouping label.
    pub fn label(&self) -> &str {
        match self {
            Self::Engine => "engine",
            Self::Twin { display_name, .. } => display_name,
        }
    }
}

/// Verified availability of one declared source, captured on an I/O worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatasetInputState {
    /// The source has not been installed.
    Missing,
    /// The source satisfies its declared integrity contract.
    Ready,
    /// A present source or its address fails validation.
    Invalid(String),
}

impl DatasetInputState {
    /// Stable availability token for language-neutral consumers.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Ready => "ready",
            Self::Invalid(_) => "invalid",
        }
    }
    /// The owner's validation failure, when present.
    pub fn error(&self) -> Option<&str> {
        match self {
            Self::Invalid(detail) => Some(detail),
            _ => None,
        }
    }
}

/// Source identity and availability, independent of processing lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatasetInput {
    /// Stable input name within the declaration.
    pub id: String,
    /// Human-readable source name.
    pub name: String,
    /// Authored download URL.
    pub url: String,
    /// Last verified source availability.
    pub state: DatasetInputState,
}

/// One declared dataset, its artifact identity, and its live state.
#[derive(Debug, Clone)]
pub struct DatasetEntry {
    /// Globally unique dataset id.
    pub id: String,
    /// Manifest key.
    pub key: String,
    /// Manifest group.
    pub group: String,
    /// Declaration scope.
    pub scope: DatasetScope,
    /// Human-readable name.
    pub name: String,
    /// Whether onboarding should recommend it.
    pub recommended: bool,
    /// Download destination.
    pub path: PathBuf,
    /// Scope-relative delivered artifact path.
    pub artifact_rel: String,
    /// Live lifecycle state.
    pub state: DatasetState,
    /// Source facts captured at admission and preparation boundaries.
    pub inputs: Vec<DatasetInput>,
    /// Full declaration, including domain metadata.
    pub spec: AssetEntry,
    read_roots: Vec<PathBuf>,
    sources: Vec<(PathBuf, lunco_storage::FilePathIdentity)>,
    output: Option<(PathBuf, lunco_storage::FilePathIdentity)>,
}

fn artifact_present(spec: &AssetEntry, path: &Path, source_path: Option<&Path>) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (spec, path, source_path);
        false
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        match &spec.process {
            Some(process) => processed_output_present(path, process, source_path),
            None => installed_destination_present(spec, path),
        }
    }
}

impl DatasetEntry {
    /// Whether every declared download has passed integrity verification.
    pub fn inputs_ready(&self) -> bool {
        !self.inputs.is_empty()
            && self
                .inputs
                .iter()
                .all(|input| input.state == DatasetInputState::Ready)
    }
    /// Inspect all declared downloads on the owning I/O worker.
    pub fn inspect_inputs(&self) -> Vec<DatasetInput> {
        let mut declarations = vec![("source", &self.spec)];
        if let Some(pipeline) = self.spec.process.as_ref().and_then(|p| p.pipeline.as_ref()) {
            declarations.extend(
                pipeline
                    .sources
                    .iter()
                    .map(|(id, spec)| (id.as_str(), spec)),
            );
        }
        declarations
            .into_iter()
            .map(|(id, spec)| {
                #[cfg(not(target_arch = "wasm32"))]
                let state = match crate::source_read_path(
                    spec,
                    &self.scope.cache_root(spec.shared),
                    &self.read_roots,
                ) {
                    Ok(_) => DatasetInputState::Ready,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        DatasetInputState::Missing
                    }
                    Err(e) => DatasetInputState::Invalid(e.to_string()),
                };
                #[cfg(target_arch = "wasm32")]
                let state = DatasetInputState::Missing;
                DatasetInput {
                    id: id.to_owned(),
                    name: spec.name.clone(),
                    url: spec.url.clone(),
                    state,
                }
            })
            .collect()
    }

    /// Explain why the selected artifact cannot be consumed after preparation.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn delivery_error(&self) -> String {
        let selected = lunco_assets_core::resolve_native_asset(
            &self.read_roots,
            Path::new(&self.artifact_rel),
            None,
        );
        if let Ok(Some((root, path))) = selected {
            if let Some(process) = &self.spec.process {
                let owner = match &self.scope {
                    DatasetScope::Twin { root, .. } => Some(root.as_path()),
                    DatasetScope::Engine => None,
                };
                if let Ok(output) = process_output_path(
                    process,
                    Some(&self.scope.cache_root(self.spec.shared)),
                    owner,
                ) {
                    if path != output && root == 0 {
                        return format!(
                            "Authored override '{}' is incomplete and takes precedence over prepared output '{}'. Remove or replace the override, then retry.",
                            path.display(),
                            output.display()
                        );
                    }
                }
            }
            return format!(
                "Prepared artifact '{}' is incomplete, invalid, or out of date. Check the declared artifacts and input integrity, then prepare again.",
                path.display()
            );
        }
        let location = self
            .artifact_uri()
            .unwrap_or_else(|| format!("{} in Twin {}", self.artifact_rel, self.scope.label()));
        format!("Prepared artifact {location} is missing. Prepare this dataset before loading it.")
    }

    /// Exact read roots validated with this declaration on its scan worker.
    pub fn read_roots(&self) -> &[PathBuf] {
        &self.read_roots
    }

    /// Validate the first artifact selected by the runtime reader, including
    /// its receipt and current inputs. Run on a scan/preparation worker.
    pub fn installed_artifact(&self) -> Result<Option<PathBuf>, String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let relative = lunco_assets_path::relative_path(&self.artifact_rel)
                .ok_or_else(|| format!("unsafe artifact path `{}`", self.artifact_rel))?;
            let Some((_, path)) =
                lunco_assets_core::resolve_native_asset(&self.read_roots, &relative, None)
                    .map_err(|error| error.to_string())?
            else {
                return Ok(None);
            };
            if !artifact_present(&self.spec, &path, None) {
                return Ok(None);
            }
            if let Some(process) = &self.spec.process {
                if process.pipeline.is_some() {
                    let cache = self.scope.cache_root(self.spec.shared);
                    let owner = match &self.scope {
                        DatasetScope::Twin { root, .. } => Some(root.as_path()),
                        DatasetScope::Engine => None,
                    };
                    let source = match crate::source_read_path(&self.spec, &cache, &self.read_roots)
                    {
                        Ok(source) => source,
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidData
                            ) =>
                        {
                            return Ok(None);
                        }
                        Err(error) => return Err(error.to_string()),
                    };
                    if !crate::pipeline::inputs_current(&path, process, &source, &cache, owner) {
                        return Ok(None);
                    }
                }
            }
            Ok(Some(path))
        }
        #[cfg(target_arch = "wasm32")]
        Ok(None)
    }

    /// Resolve the consumer-facing asset URI using the stable source identity.
    /// Offline verification has no live Twin scope and cannot produce an address.
    pub fn artifact_uri(&self) -> Option<String> {
        match &self.scope {
            DatasetScope::Engine => Some(lunco_assets_path::uri(
                lunco_assets_core::LUNCO_SCHEME,
                &self.artifact_rel,
            )),
            DatasetScope::Twin {
                source_identity,
                authority,
                ..
            } => authority.as_ref().map(|_| {
                lunco_assets_path::uri(
                    lunco_assets_core::TWIN_SCHEME,
                    &format!("{source_identity}/{}", self.artifact_rel),
                )
            }),
        }
    }
}

fn artifact_rel_of(
    entry: &AssetEntry,
    scope: &DatasetScope,
    destination: &Path,
) -> Result<String, std::io::Error> {
    if let Some(process) = &entry.process {
        let twin_root = match scope {
            DatasetScope::Twin { root, .. } => Some(root.as_path()),
            DatasetScope::Engine => None,
        };
        let cache_root = scope.cache_root(entry.shared);
        let absolute = process_output_path(process, Some(&cache_root), twin_root)?;
        if process.output_root == "assets" {
            if matches!(scope, DatasetScope::Twin { .. }) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "process output_root=\"assets\" is engine-owned and cannot deliver a Twin artifact",
                ));
            }
            return Ok(process.output.clone());
        }
        if process.output_root == "twin" && !matches!(scope, DatasetScope::Twin { .. }) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "process output_root=\"twin\" requires a Twin-scoped dataset",
            ));
        }
        if process.output_root == "twin" {
            return Ok(process.output.clone());
        }
        return absolute
            .strip_prefix(&cache_root)
            .map(lunco_assets_path::slashed)
            .map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "processed cache output {} is outside its owning cache {}",
                        absolute.display(),
                        cache_root.display()
                    ),
                )
            });
    }
    destination
        .strip_prefix(scope.cache_root(entry.shared))
        .map(lunco_assets_path::slashed)
        .map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "download output {} is outside its owning cache for scope {}",
                    destination.display(),
                    scope.label()
                ),
            )
        })
}

/// User-visible telemetry name for dataset declaration/provision failures.
pub const DATASET_FAILED: &str = "DATASET_FAILED";

/// Create the canonical dataset failure telemetry event.
pub fn dataset_failed(detail: impl Into<String>) -> lunco_telemetry_core::TelemetryEvent {
    lunco_telemetry_core::TelemetryEvent {
        name: DATASET_FAILED.into(),
        source: 0,
        severity: lunco_telemetry_core::Severity::Error,
        data: lunco_telemetry_core::TelemetryValue::String(detail.into()),
        timestamp: 0.0,
        sim_secs: 0.0,
        sim_tick: 0,
    }
}

/// Report a discrete provisioning phase through the shared diagnostic stream.
/// Byte samples remain registry facts; they do not create one log row per chunk.
pub fn dataset_state_changed(
    id: &str,
    name: &str,
    state: &DatasetState,
) -> lunco_telemetry_core::TelemetryEvent {
    let detail = match state {
        DatasetState::Failed(error) => return dataset_failed(format!("{name} ({id}): {error}")),
        DatasetState::Processing { kind } => format!("{name}: preparing {kind}"),
        DatasetState::Downloading { source, .. } => format!("{name}: downloading {source}"),
        DatasetState::Queued => format!("{name}: queued for preparation"),
        DatasetState::Installed => format!("{name}: data ready"),
        DatasetState::Cancelled => {
            format!("{name}: cancelled; resume in Settings > Data & libraries")
        }
        _ => format!("{name} ({id}): {}", state.kind()),
    };
    lunco_telemetry_core::TelemetryEvent {
        name: format!("DATASET_{}", state.kind().to_ascii_uppercase()),
        source: 0,
        severity: lunco_telemetry_core::Severity::Info,
        data: lunco_telemetry_core::TelemetryValue::String(detail),
        timestamp: 0.0,
        sim_secs: 0.0,
        sim_tick: 0,
    }
}

/// Registry of all declared engine and Twin datasets.
#[derive(Resource, Default)]
pub struct DatasetRegistry {
    entries: Vec<DatasetEntry>,
    scanned_scopes: Vec<DatasetScope>,
    pending_failures: Vec<String>,
}

impl DatasetRegistry {
    /// Register an engine-scoped manifest.
    pub fn register(&mut self, assets_toml: &str, group: &str) -> usize {
        self.register_scoped(assets_toml, group, DatasetScope::Engine)
    }

    /// Register a manifest under an explicit scope on the owning I/O worker.
    /// Native destination identities are prepared here; admission compares them without I/O.
    pub fn register_scoped(
        &mut self,
        assets_toml: &str,
        group: &str,
        scope: DatasetScope,
    ) -> usize {
        let manifest: AssetManifest = match assets_toml.parse() {
            Ok(manifest) => manifest,
            Err(error) => {
                self.record_failure(format!("{group}: Assets.toml parse failed: {error}"));
                return 0;
            }
        };
        let mut added = 0;
        for (key, spec) in manifest.assets {
            if spec.package_only {
                continue;
            }
            let id = dataset_id(&scope, group, &key);
            if self.entries.iter().any(|entry| entry.id == id) {
                self.record_failure(format!(
                    "duplicate dataset key '{key}' within scope '{}' — ignored",
                    scope.label()
                ));
                continue;
            }
            let destination_root = scope.cache_root(spec.shared);
            let path = match entry_dest_path(&spec, Some(&destination_root)) {
                Ok(path) => path,
                Err(error) => {
                    self.record_failure(format!(
                        "dataset '{key}' in scope '{}' has an invalid destination: {error}",
                        scope.label()
                    ));
                    continue;
                }
            };
            let artifact_rel = match artifact_rel_of(&spec, &scope, &path) {
                Ok(relative) => relative,
                Err(error) => {
                    self.record_failure(format!(
                        "dataset '{key}' in scope '{}' has an invalid processed output: {error}",
                        scope.label()
                    ));
                    continue;
                }
            };
            let output_path = if let Some(process) = &spec.process {
                let twin_root = match &scope {
                    DatasetScope::Twin { root, .. } => Some(root.as_path()),
                    DatasetScope::Engine => None,
                };
                let output_path = match process_output_path(
                    process,
                    Some(&scope.cache_root(spec.shared)),
                    twin_root,
                ) {
                    Ok(path) => path,
                    Err(error) => {
                        self.record_failure(format!(
                            "dataset '{key}' in scope '{}' has an invalid processed output: {error}",
                            scope.label()
                        ));
                        continue;
                    }
                };
                Some(output_path)
            } else {
                None
            };
            let identities = (|| -> Result<_, lunco_storage::StorageError> {
                let mut sources = vec![(
                    path.clone(),
                    lunco_storage::FilePathIdentity::prepare(&path)?,
                )];
                if let Some(pipeline) = spec
                    .process
                    .as_ref()
                    .and_then(|process| process.pipeline.as_ref())
                {
                    for source in pipeline.sources.values() {
                        let input = entry_dest_path(source, Some(&destination_root))
                            .map_err(lunco_storage::StorageError::Io)?;
                        sources.push((
                            input.clone(),
                            lunco_storage::FilePathIdentity::prepare(&input)?,
                        ));
                    }
                }
                let output = output_path
                    .map(|path| {
                        lunco_storage::FilePathIdentity::prepare(&path)
                            .map(|identity| (path, identity))
                    })
                    .transpose()?;
                Ok((sources, output))
            })();
            let (sources, output) = match identities {
                Ok(identities) => identities,
                Err(error) => {
                    self.record_failure(format!(
                        "dataset '{key}' destination identity failed: {error}"
                    ));
                    continue;
                }
            };
            if let Err(conflict) = self.process_output_conflict(&key, &sources, output.as_ref()) {
                self.record_failure(conflict);
                continue;
            }
            let read_roots = match scope.read_roots() {
                Ok(roots) => roots,
                Err(error) => {
                    self.record_failure(format!("dataset '{key}' read root failed: {error}"));
                    continue;
                }
            };
            let mut entry = DatasetEntry {
                id,
                key: key.clone(),
                group: group.to_owned(),
                scope: scope.clone(),
                name: spec.name.clone(),
                recommended: spec.recommended,
                path,
                artifact_rel,
                state: DatasetState::Missing,
                inputs: Vec::new(),
                spec,
                read_roots,
                sources,
                output,
            };
            entry.inputs = entry.inspect_inputs();
            entry.state = match entry.installed_artifact() {
                Ok(Some(_)) => DatasetState::Installed,
                Ok(None) => DatasetState::Missing,
                Err(error) => DatasetState::Failed(error),
            };
            self.entries.push(entry);
            added += 1;
        }
        added
    }

    /// Merge a registry prepared on a worker while preserving conflicts with
    /// declarations that were admitted on the main thread in the meantime.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn merge_prepared(&mut self, mut prepared: Self) -> usize {
        self.pending_failures.append(&mut prepared.pending_failures);
        for scope in prepared.scanned_scopes.drain(..) {
            if !self.is_scope_scanned(&scope) {
                self.scanned_scopes.push(scope);
            }
        }
        let mut added = 0;
        for entry in prepared.entries.drain(..) {
            if self.entries.iter().any(|current| current.id == entry.id) {
                self.record_failure(format!(
                    "duplicate dataset key '{}' within scope '{}' — ignored",
                    entry.key,
                    entry.scope.label()
                ));
                continue;
            }
            if let Err(conflict) =
                self.process_output_conflict(&entry.key, &entry.sources, entry.output.as_ref())
            {
                self.record_failure(conflict);
                continue;
            }
            self.entries.push(entry);
            added += 1;
        }
        added
    }

    fn process_output_conflict(
        &self,
        key: &str,
        sources: &[(PathBuf, lunco_storage::FilePathIdentity)],
        output: Option<&(PathBuf, lunco_storage::FilePathIdentity)>,
    ) -> Result<(), String> {
        let overlaps = |a: &lunco_storage::FilePathIdentity,
                        b: &lunco_storage::FilePathIdentity| {
            a.overlaps(b).map_err(|e| {
                format!("dataset '{key}' output ownership could not be established: {e}")
            })
        };
        if let Some((path, identity)) = output {
            for (source, source_identity) in sources {
                if overlaps(identity, source_identity)? {
                    return Err(format!(
                        "dataset '{key}' process output {} overlaps its downloaded source {}",
                        path.display(),
                        source.display()
                    ));
                }
            }
        }
        for entry in &self.entries {
            if let Some((path, identity)) = output {
                for (source, source_identity) in &entry.sources {
                    if overlaps(identity, source_identity)? {
                        return Err(format!(
                            "dataset '{key}' process output {} overlaps dataset '{}' source {}",
                            path.display(),
                            entry.key,
                            source.display()
                        ));
                    }
                }
                if let Some((other, other_identity)) = &entry.output {
                    if overlaps(identity, other_identity)? {
                        return Err(format!(
                            "dataset '{key}' process output {} overlaps dataset '{}' process output {}",
                            path.display(),
                            entry.key,
                            other.display()
                        ));
                    }
                }
            }
            if let Some((path, identity)) = &entry.output {
                for (source, source_identity) in sources {
                    if overlaps(identity, source_identity)? {
                        return Err(format!(
                            "dataset '{key}' source {} overlaps dataset '{}' process output {}",
                            source.display(),
                            entry.key,
                            path.display()
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Scan and register an opened Twin's `Assets.toml`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn scan_twin(
        &mut self,
        source_identity: &str,
        authority: Option<&str>,
        display_name: &str,
        root: &Path,
    ) -> usize {
        let scope = DatasetScope::Twin {
            source_identity: source_identity.to_owned(),
            authority: authority.map(str::to_owned),
            display_name: display_name.to_owned(),
            root: root.to_path_buf(),
        };
        self.forget_scope(&scope);
        self.scanned_scopes.push(scope.clone());
        let path = root.join("Assets.toml");
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return 0,
            Err(error) => {
                self.record_failure(format!(
                    "cannot read Twin manifest {}: {error}",
                    path.display()
                ));
                return 0;
            }
        };
        self.register_scoped(&text, display_name, scope)
    }

    /// Record a registry-owned failure for the runtime telemetry drain.
    pub fn record_failure(&mut self, detail: impl Into<String>) {
        self.pending_failures.push(detail.into());
    }

    /// Drain failures raised by registration or discovery.
    pub fn take_pending_failures(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_failures)
    }

    /// Remove all declarations for one scope.
    pub fn forget_scope(&mut self, scope: &DatasetScope) {
        self.entries.retain(|entry| &entry.scope != scope);
        self.scanned_scopes.retain(|known| known != scope);
    }

    /// Find all Twin scopes backed by one root, including empty scanned scopes.
    pub fn scopes_for_root(&self, root: &Path) -> Vec<DatasetScope> {
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
        scopes
    }

    /// Every declared dataset, in registration order.
    pub fn entries(&self) -> &[DatasetEntry] {
        &self.entries
    }

    /// Scopes whose manifests completed discovery.
    pub fn scanned_scopes(&self) -> &[DatasetScope] {
        &self.scanned_scopes
    }

    /// Whether a scope completed discovery.
    pub fn is_scope_scanned(&self, scope: &DatasetScope) -> bool {
        self.scanned_scopes
            .iter()
            .any(|scanned| scanned.same_storage(scope))
    }

    /// Mark discovery complete even when a scope declares no downloads.
    pub fn mark_scope_scanned(&mut self, scope: DatasetScope) {
        if !self.is_scope_scanned(&scope) {
            self.scanned_scopes.push(scope);
        }
    }

    /// Resolve a consumed logical URI through the same scope/artifact matcher.
    /// This is a read-side operation over admitted mount metadata; it performs no I/O.
    pub fn declared_uri<'a>(
        &'a self,
        uri: &str,
        roots: Option<&lunco_assets_core::TwinRoots>,
    ) -> Result<Option<&'a DatasetEntry>, String> {
        let Some((scope, relative)) = dataset_scope_for_uri(uri, roots)? else {
            return Ok(None);
        };
        Ok(self.declared_artifact(&scope, &relative))
    }

    /// Discovery must settle before a runtime loader can classify a missing file.
    pub fn uri_scope_scanned(
        &self,
        uri: &str,
        roots: Option<&lunco_assets_core::TwinRoots>,
    ) -> Result<bool, String> {
        Ok(dataset_scope_for_uri(uri, roots)?
            .is_none_or(|(scope, _)| self.is_scope_scanned(&scope)))
    }

    /// Find the declaration delivering a file or an explicit bundle member.
    pub fn declared_artifact(
        &self,
        scope: &DatasetScope,
        relative: &Path,
    ) -> Option<&DatasetEntry> {
        let relative = lunco_assets_path::slashed(relative)
            .trim_start_matches('/')
            .to_owned();
        self.entries.iter().find(|entry| {
            if !entry.scope.same_storage(scope) {
                return false;
            }
            if entry.artifact_rel == relative {
                return true;
            }
            let Some(member) = Path::new(&relative).strip_prefix(&entry.artifact_rel).ok() else {
                return false;
            };
            if let Some(pipeline) = entry
                .spec
                .process
                .as_ref()
                .and_then(|process| process.pipeline.as_ref())
            {
                return pipeline
                    .artifacts
                    .iter()
                    .any(|artifact| Path::new(&artifact.path) == member);
            }
            entry.spec.process.is_none() && crate::archive_extension(&entry.spec.url).is_some()
        })
    }

    /// Look up one dataset's state.
    pub fn state(&self, id: &str) -> Option<&DatasetState> {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| &entry.state)
    }

    /// Find an installed dataset by id.
    pub fn installed(&self, id: &str) -> Option<&DatasetEntry> {
        self.entries
            .iter()
            .find(|entry| entry.id == id && entry.state.is_installed())
    }

    /// Iterate over entries that may be requested.
    pub fn missing(&self) -> impl Iterator<Item = &DatasetEntry> {
        self.entries.iter().filter(|entry| {
            matches!(
                entry.state,
                DatasetState::Missing | DatasetState::Failed(_) | DatasetState::Cancelled
            )
        })
    }

    /// Look up one complete entry for a provisioning worker.
    pub fn entry(&self, id: &str) -> Option<&DatasetEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    /// Mark an explicit request as in flight.
    pub fn request(&mut self, id: &str) -> bool {
        let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) else {
            return false;
        };
        if matches!(
            entry.state,
            DatasetState::Installed
                | DatasetState::Queued
                | DatasetState::Downloading { .. }
                | DatasetState::Processing { .. }
                | DatasetState::Cancelling
        ) {
            return false;
        }
        entry.state = DatasetState::Downloading {
            source: entry.name.clone(),
            bytes_done: 0,
            bytes_total: 0,
        };
        true
    }

    /// Mark a manifest-declared processing pipeline as in flight without
    /// downloading its source again. Installed datasets may be processed again;
    /// the processor's bake key decides whether any work is needed.
    pub fn process(&mut self, id: &str) -> bool {
        let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) else {
            return false;
        };
        let Some(process) = &entry.spec.process else {
            return false;
        };
        if matches!(
            entry.state,
            DatasetState::Queued
                | DatasetState::Downloading { .. }
                | DatasetState::Processing { .. }
                | DatasetState::Cancelling
        ) {
            return false;
        }
        entry.state = DatasetState::Processing {
            kind: process.kind.clone(),
        };
        true
    }

    /// Mark an explicit cancellation request as unwinding.
    pub fn cancel(&mut self, id: &str) -> bool {
        let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) else {
            return false;
        };
        if !matches!(
            entry.state,
            DatasetState::Queued
                | DatasetState::Downloading { .. }
                | DatasetState::Processing { .. }
        ) {
            return false;
        }
        entry.state = DatasetState::Cancelling;
        true
    }

    /// Apply a worker state if the entry is still part of this lifecycle.
    pub fn set_state(&mut self, id: &str, state: DatasetState) -> bool {
        let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) else {
            return false;
        };
        if entry.state == state {
            return false;
        }
        // Once cancellation is accepted, an already queued progress sample
        // cannot make the operation look active again. Only its terminal
        // worker result may complete this transition.
        if entry.state == DatasetState::Cancelling
            && matches!(
                state,
                DatasetState::Queued
                    | DatasetState::Downloading { .. }
                    | DatasetState::Processing { .. }
            )
        {
            return false;
        }
        entry.state = state;
        true
    }

    /// Commit input facts prepared by the current scope's I/O worker.
    pub fn set_inputs(&mut self, id: &str, inputs: Vec<DatasetInput>) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
            entry.inputs = inputs;
        }
    }
}

/// Build the stable registry id for a scoped manifest entry.
pub fn dataset_id(scope: &DatasetScope, group: &str, key: &str) -> String {
    match scope {
        DatasetScope::Engine => format!("engine/{group}/{key}"),
        DatasetScope::Twin {
            source_identity, ..
        } => format!("twin/{source_identity}/{key}"),
    }
}

/// User intent to start one dataset operation.
#[Command]
pub struct RequestDataset {
    /// Globally unique dataset id.
    pub id: String,
}

/// Process one declared dataset's available source without requesting a download.
///
/// The manifest supplies both the source identity and processing configuration;
/// this command only selects the declared dataset. The native processor checks
/// the content bake key and skips work when the output is already current.
#[Command]
pub struct ProcessDataset {
    /// Globally unique dataset id.
    pub id: String,
    /// Rerun all steps even when the published bake key matches. Defaults to false.
    #[serde(default)]
    #[reflect(default)]
    pub force: bool,
}

/// User intent to cancel one dataset operation.
#[Command]
pub struct CancelDataset {
    /// Globally unique dataset id.
    pub id: String,
}

/// A dataset scope completed discovery.
#[derive(Event, Clone, Debug)]
pub struct DatasetScopeReady {
    /// Discovered scope.
    pub scope: DatasetScope,
}

/// A Twin scope was removed after its workers were retired.
#[derive(Event, Clone, Debug)]
pub struct DatasetScopeRemoved {
    /// Removed scope.
    pub scope: DatasetScope,
}

/// A delivered dataset artifact became ready.
#[derive(Event, Clone, Debug)]
pub struct DatasetInstalled {
    /// Globally unique dataset id.
    pub id: String,
    /// Owning scope.
    pub scope: DatasetScope,
    /// Consumer-facing asset URI.
    pub artifact_uri: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const MANIFEST: &str = r#"
[demo_vectors]
name = "Demo vectors"
url = "https://example.invalid/vectors.csv"
dest = "data/demo.csv"
"#;

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn native_resolution_contract_keeps_dataset_and_reader_precedence_equal() {
        let root = tempfile::tempdir().unwrap();
        let authored = root.path().join("data/demo.csv");
        let cached = lunco_assets_core::twin_cache_dir(root.path()).join("data/demo.csv");
        std::fs::create_dir_all(authored.parent().unwrap()).unwrap();
        std::fs::create_dir_all(cached.parent().unwrap()).unwrap();
        std::fs::write(&authored, b"").unwrap();
        std::fs::write(&cached, b"valid cached artifact").unwrap();
        let scope = DatasetScope::Twin {
            source_identity: "fixture".into(),
            authority: Some("opaque-authority".into()),
            display_name: "fixture".into(),
            root: root.path().to_path_buf(),
        };
        let mut invalid = DatasetRegistry::default();
        assert_eq!(
            invalid.register_scoped(MANIFEST, "fixture", scope.clone()),
            1
        );
        assert_eq!(
            invalid.entries()[0].state,
            DatasetState::Missing,
            "an invalid authored artifact must not select a different cache copy"
        );
        std::fs::write(&authored, b"valid authored artifact").unwrap();
        let mut valid = DatasetRegistry::default();
        assert_eq!(valid.register_scoped(MANIFEST, "fixture", scope), 1);
        assert_eq!(valid.entries()[0].state, DatasetState::Installed);
        assert_eq!(
            valid.entries()[0].installed_artifact().unwrap(),
            Some(std::fs::canonicalize(authored).unwrap())
        );
    }

    #[test]
    fn registration_is_local_and_scoped() {
        let mut registry = DatasetRegistry::default();
        assert_eq!(registry.register(MANIFEST, "demo"), 1);
        let entry = &registry.entries()[0];
        assert_eq!(entry.id, "engine/demo/demo_vectors");
        assert_eq!(entry.state, DatasetState::Missing);
        assert_eq!(entry.scope.id(), "engine");
        assert_eq!(
            DatasetScope::Twin {
                source_identity: "stable-fixture".into(),
                authority: Some("opaque-authority".into()),
                display_name: "Named fixture".into(),
                root: PathBuf::from("/fixture"),
            }
            .id(),
            "twin:stable-fixture"
        );
        assert_eq!(
            entry.artifact_uri().as_deref(),
            Some("lunco://data/demo.csv")
        );

        let offline_scope = DatasetScope::Twin {
            source_identity: "stable-fixture".into(),
            authority: None,
            display_name: "Named fixture".into(),
            root: PathBuf::from("/fixture"),
        };
        let mut offline_registry = DatasetRegistry::default();
        assert_eq!(
            offline_registry.register_scoped(MANIFEST, "Named fixture", offline_scope),
            1
        );
        let offline_entry = &offline_registry.entries()[0];
        assert_eq!(offline_entry.id, "twin/stable-fixture/demo_vectors");
        assert_eq!(offline_entry.scope.id(), "twin:stable-fixture");
        assert_eq!(offline_entry.scope.label(), "Named fixture");
        assert_eq!(offline_entry.artifact_uri(), None);
        assert_eq!(
            dataset_id(
                &DatasetScope::Twin {
                    source_identity: "stable-fixture".into(),
                    authority: Some("fresh-authority".into()),
                    display_name: "Renamed presentation label".into(),
                    root: PathBuf::from("/fixture"),
                },
                "ignored-group",
                "demo_vectors"
            ),
            offline_entry.id
        );
    }

    #[test]
    fn duplicate_ids_and_invalid_manifests_are_visible() {
        let mut registry = DatasetRegistry::default();
        assert_eq!(registry.register(MANIFEST, "demo"), 1);
        assert_eq!(registry.register(MANIFEST, "demo"), 0);
        assert_eq!(registry.register("not = [toml", "broken"), 0);
        assert_eq!(registry.entries().len(), 1);
        assert_eq!(registry.take_pending_failures().len(), 2);
    }

    #[test]
    fn lifecycle_requests_only_change_state() {
        let mut registry = DatasetRegistry::default();
        registry.register(MANIFEST, "demo");
        let id = registry.entries()[0].id.clone();
        assert!(registry.request(&id));
        assert!(matches!(
            registry.state(&id),
            Some(DatasetState::Downloading { .. })
        ));
        assert!(registry.cancel(&id));
        assert_eq!(registry.state(&id), Some(&DatasetState::Cancelling));
        assert!(!registry.set_state(
            &id,
            DatasetState::Downloading {
                source: "Fixture".to_owned(),
                bytes_done: 1,
                bytes_total: 2,
            }
        ));
        assert!(!registry.set_state(&id, DatasetState::Processing { kind: "dem".into() }));
        assert_eq!(registry.state(&id), Some(&DatasetState::Cancelling));
        assert!(registry.set_state(&id, DatasetState::Cancelled));
        assert!(registry.request(&id));
    }

    #[test]
    fn delivered_paths_are_scope_relative() {
        let mut registry = DatasetRegistry::default();
        let scope = DatasetScope::Twin {
            source_identity: "school".into(),
            authority: Some("opaque-authority".into()),
            display_name: "School".into(),
            root: PathBuf::from("/twins/school"),
        };
        assert_eq!(
            registry.register_scoped(MANIFEST, "school", scope.clone()),
            1
        );
        assert!(
            registry
                .declared_artifact(&scope, Path::new("data/demo.csv"))
                .is_some()
        );
    }

    #[test]
    fn overlapping_process_outputs_are_rejected_during_registration() {
        let manifest = r#"
[base_dem]
name = "Base elevation"
url = "https://example.invalid/dem.tif"

[base_dem.process]
kind = "dem"
output_root = "cache"
output = "terrain/site"

[detail_albedo]
name = "Material albedo"
url = "https://example.invalid/albedo.tif"

[detail_albedo.process]
kind = "albedo"
output_root = "cache"
output = "terrain/site/materials/textures/albedo.png"
"#;
        let scope = DatasetScope::Twin {
            source_identity: "school".into(),
            authority: Some("opaque-authority".into()),
            display_name: "School".into(),
            root: PathBuf::from("/twins/school"),
        };
        let mut registry = DatasetRegistry::default();

        assert_eq!(registry.register_scoped(manifest, "school", scope), 1);
        assert_eq!(registry.entries().len(), 1);
        assert_eq!(registry.entries()[0].key, "base_dem");
        let failures = registry.take_pending_failures();
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("detail_albedo"));
        assert!(failures[0].contains("overlaps dataset 'base_dem' process output"));
    }
}

/// A mounted storage owner retired or transferred to another live authority.
/// The worker owner must cross its commit barrier before removing declarations.
#[derive(Event, Clone, Debug)]
pub struct RetireDatasetScope {
    /// Exact retiring authority and canonical root.
    pub scope: DatasetScope,
}

fn dataset_scope_for_uri(
    uri: &str,
    roots: Option<&lunco_assets_core::TwinRoots>,
) -> Result<Option<(DatasetScope, PathBuf)>, String> {
    let (scope, relative) = if let Some(relative) = lunco_assets_core::parse_lunco_uri(uri) {
        (DatasetScope::Engine, relative)
    } else if let Some((authority, relative)) = lunco_assets_core::parse_twin_uri(uri) {
        let roots = roots.ok_or("Twin asset registry is unavailable")?;
        let root = roots
            .root_for(authority)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Twin asset authority retired: {authority}"))?;
        let display_name = roots
            .display_name_for_authority(authority)
            .map_err(|e| e.to_string())?;
        let source_identity = roots
            .logical_identity_for_authority(authority)
            .map_err(|e| e.to_string())?;
        (
            DatasetScope::Twin {
                source_identity,
                authority: Some(authority.to_owned()),
                display_name,
                root,
            },
            relative,
        )
    } else {
        return Ok(None);
    };
    let relative = lunco_assets_path::relative_path(relative)
        .ok_or_else(|| format!("Unsafe asset URI: {uri}"))?;
    Ok(Some((scope, relative)))
}

/// A runtime owner actually needs a logical asset; this is independent of catalog recommendations.
#[derive(Event, Clone, Debug)]
pub struct AssetRequired {
    /// Exact admitted scene lease. Retired scene completions cannot add requirements.
    pub scene: Entity,
    /// Canonical URI used by the real loader.
    pub uri: String,
}

/// Assets consumed dynamically during this admitted scene. Cleared by scene teardown.
#[derive(Resource, Default)]
pub struct RuntimeAssetRequirements {
    /// Exact scene lease.
    pub scene: Option<Entity>,
    /// Actual runtime use, rather than a separately authored importance list.
    pub uris: std::collections::BTreeSet<String>,
}
impl lunco_core::SceneResource for RuntimeAssetRequirements {}

/// Declare a concrete runtime asset dependency without initiating a network request.
/// Missing declared data holds simulation and opens the shared preparation window.
#[Command]
pub struct RequireAsset {
    /// Logical asset URI consumed by the caller, normally twin:// or lunco://.
    pub uri: String,
}
