//! Shared dataset identity, state, and Bevy-facing lifecycle events.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use lunco_core::Command;

use crate::{entry_dest_path, process_output_path, AssetEntry, AssetManifest};
#[cfg(not(target_arch = "wasm32"))]
use crate::{installed_destination_present, processed_output_present};

/// What a declared dataset is currently doing.
#[derive(Debug, Clone, PartialEq)]
pub enum DatasetState {
    /// Declared, not on disk.
    Missing,
    /// A user-requested download is running.
    Downloading {
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
        /// `twin://` authority.
        name: String,
        /// Absolute Twin root.
        root: PathBuf,
    },
}

impl DatasetScope {
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
    pub fn read_roots(&self) -> Vec<PathBuf> {
        match self {
            Self::Engine => lunco_assets_core::library_roots(&lunco_assets_core::assets_dir_abs()),
            Self::Twin { root, .. } => vec![
                root.clone(),
                lunco_assets_core::twin_cache_dir(root),
                lunco_assets_core::cache_dir(),
            ],
        }
    }

    /// Human-readable grouping label.
    pub fn label(&self) -> &str {
        match self {
            Self::Engine => "engine",
            Self::Twin { name, .. } => name,
        }
    }
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
    /// Full declaration, including domain metadata.
    pub spec: AssetEntry,
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
    /// Resolve the first readable copy of the delivered artifact.
    pub fn artifact_path(&self) -> PathBuf {
        for root in self.scope.read_roots() {
            let candidate = root.join(&self.artifact_rel);
            if artifact_present(&self.spec, &candidate, None) {
                return candidate;
            }
        }
        self.scope
            .cache_root(self.spec.shared)
            .join(&self.artifact_rel)
    }

    fn source_path_for_root(&self, artifact_root: &Path) -> Option<PathBuf> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = artifact_root;
            None
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let source_rel = self
                .scope
                .read_roots()
                .iter()
                .find_map(|root| self.path.strip_prefix(root).ok().map(Path::to_path_buf));
            source_rel
                .map(|relative| artifact_root.join(relative))
                .filter(|path| path.is_file())
                .or_else(|| self.path.is_file().then(|| self.path.clone()))
        }
    }

    /// Resolve the consumer-facing asset URI.
    pub fn artifact_uri(&self) -> String {
        match &self.scope {
            DatasetScope::Engine => {
                lunco_assets_path::uri(lunco_assets_core::LUNCO_SCHEME, &self.artifact_rel)
            }
            DatasetScope::Twin { name, .. } => {
                lunco_assets_core::twin_uri(name, &self.artifact_rel)
            }
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

    /// Register a manifest under an explicit scope.
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
            if let Some(conflict) =
                self.process_output_conflict(&key, &path, output_path.as_deref())
            {
                self.record_failure(conflict);
                continue;
            }
            let state = if scope
                .read_roots()
                .iter()
                .any(|root| artifact_present(&spec, &root.join(&artifact_rel), None))
            {
                DatasetState::Installed
            } else {
                DatasetState::Missing
            };
            self.entries.push(DatasetEntry {
                id,
                key: key.clone(),
                group: group.to_owned(),
                scope: scope.clone(),
                name: spec.name.clone(),
                recommended: spec.recommended,
                path,
                artifact_rel,
                state,
                spec,
            });
            added += 1;
        }
        added
    }

    fn process_output_conflict(
        &self,
        key: &str,
        source_path: &Path,
        output_path: Option<&Path>,
    ) -> Option<String> {
        if let Some(output_path) = output_path {
            if paths_overlap(source_path, output_path) {
                return Some(format!(
                    "dataset '{key}' process output {} overlaps its downloaded source {}",
                    output_path.display(),
                    source_path.display()
                ));
            }
        }

        for entry in &self.entries {
            let entry_output = if let Some(process) = &entry.spec.process {
                let twin_root = match &entry.scope {
                    DatasetScope::Twin { root, .. } => Some(root.as_path()),
                    DatasetScope::Engine => None,
                };
                match process_output_path(
                    process,
                    Some(&entry.scope.cache_root(entry.spec.shared)),
                    twin_root,
                ) {
                    Ok(path) => Some(path),
                    Err(error) => {
                        return Some(format!(
                            "dataset '{}' has an invalid process output while checking dataset '{key}': {error}",
                            entry.key
                        ));
                    }
                }
            } else {
                None
            };

            if let Some(output_path) = output_path {
                if paths_overlap(output_path, &entry.path) {
                    return Some(format!(
                        "dataset '{key}' process output {} overlaps dataset '{}' source {}",
                        output_path.display(),
                        entry.key,
                        entry.path.display()
                    ));
                }
            }

            if let Some(entry_output) = entry_output {
                if let Some(output_path) = output_path {
                    if paths_overlap(output_path, &entry_output) {
                        return Some(format!(
                            "dataset '{key}' process output {} overlaps dataset '{}' process output {}",
                            output_path.display(),
                            entry.key,
                            entry_output.display()
                        ));
                    }
                }
                if paths_overlap(source_path, &entry_output) {
                    return Some(format!(
                        "dataset '{key}' source {} overlaps dataset '{}' process output {}",
                        source_path.display(),
                        entry.key,
                        entry_output.display()
                    ));
                }
            }
        }
        None
    }

    /// Scan and register an opened Twin's `Assets.toml`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn scan_twin(&mut self, name: &str, root: &Path) -> usize {
        let scope = DatasetScope::Twin {
            name: name.to_owned(),
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
        self.register_scoped(&text, name, scope)
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

    /// Recheck all entries against the on-disk artifact contract.
    pub fn refresh_installed_state(&mut self) {
        for entry in &mut self.entries {
            if matches!(
                entry.state,
                DatasetState::Downloading { .. }
                    | DatasetState::Processing { .. }
                    | DatasetState::Cancelling
            ) {
                continue;
            }
            let installed = entry.scope.read_roots().iter().any(|root| {
                let artifact = root.join(&entry.artifact_rel);
                artifact_present(
                    &entry.spec,
                    &artifact,
                    entry.source_path_for_root(root).as_deref(),
                )
            });
            entry.state = if installed {
                DatasetState::Installed
            } else if let DatasetState::Failed(error) = &entry.state {
                DatasetState::Failed(error.clone())
            } else {
                DatasetState::Missing
            };
        }
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
        self.scanned_scopes.iter().any(|scanned| scanned == scope)
    }

    /// Find a declaration by its delivered scope-relative artifact path.
    pub fn declared_artifact(
        &self,
        scope: &DatasetScope,
        relative: &Path,
    ) -> Option<&DatasetEntry> {
        let relative = lunco_assets_path::slashed(relative)
            .trim_start_matches('/')
            .to_owned();
        self.entries
            .iter()
            .find(|entry| &entry.scope == scope && entry.artifact_rel == relative)
    }

    /// Look up one dataset's state.
    pub fn state(&self, id: &str) -> Option<&DatasetState> {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| &entry.state)
    }

    /// Resolve one dataset's delivered artifact path.
    pub fn path(&self, id: &str) -> Option<PathBuf> {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .map(DatasetEntry::artifact_path)
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
                | DatasetState::Downloading { .. }
                | DatasetState::Processing { .. }
                | DatasetState::Cancelling
        ) {
            return false;
        }
        entry.state = DatasetState::Downloading {
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
            DatasetState::Downloading { .. }
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
            DatasetState::Downloading { .. } | DatasetState::Processing { .. }
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
        entry.state = state;
        true
    }
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

/// Build the stable registry id for a scoped manifest entry.
pub fn dataset_id(scope: &DatasetScope, group: &str, key: &str) -> String {
    match scope {
        DatasetScope::Engine => format!("engine/{group}/{key}"),
        DatasetScope::Twin { name, .. } => format!("twin/{name}/{key}"),
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
    /// Resolved delivered artifact path.
    pub artifact_path: PathBuf,
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
    fn registration_is_local_and_scoped() {
        let mut registry = DatasetRegistry::default();
        assert_eq!(registry.register(MANIFEST, "demo"), 1);
        let entry = &registry.entries()[0];
        assert_eq!(entry.id, "engine/demo/demo_vectors");
        assert_eq!(entry.state, DatasetState::Missing);
        assert_eq!(entry.artifact_uri(), "lunco://data/demo.csv");
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
        assert!(registry.set_state(&id, DatasetState::Cancelled));
        assert!(registry.request(&id));
    }

    #[test]
    fn delivered_paths_are_scope_relative() {
        let mut registry = DatasetRegistry::default();
        let scope = DatasetScope::Twin {
            name: "school".into(),
            root: PathBuf::from("/twins/school"),
        };
        assert_eq!(
            registry.register_scoped(MANIFEST, "school", scope.clone()),
            1
        );
        assert!(registry
            .declared_artifact(&scope, Path::new("data/demo.csv"))
            .is_some());
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
            name: "school".into(),
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
