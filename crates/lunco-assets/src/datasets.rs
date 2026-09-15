//! Native dataset provisioning runtime.
//
//! The declarations, registry, commands, and state types live in
//! lunco_assets_datasets. This module owns only the application boundary
//! that performs downloads and native processing. Keeping workers here prevents
//! consumers that only inspect dataset state from inheriting the asset
//! processor's HTTP, archive, image, and GeoTIFF dependency closure.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use lunco_assets_datasets::{
    dataset_failed, AssetEntry, CancelDataset, DatasetEntry, DatasetInstalled, DatasetRegistry,
    DatasetScope, DatasetScopeRemoved, DatasetState, RequestDataset,
};
use lunco_core::{on_command, register_commands};

/// Cross-thread slot for progress produced by one download worker.
type StatusSlot = Arc<Mutex<Option<DatasetState>>>;

/// One owned provisioning operation.
struct DownloadHandle {
    scope: DatasetScope,
    status: StatusSlot,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    commit_gate: Arc<Mutex<()>>,
    #[cfg(not(target_arch = "wasm32"))]
    task: Option<bevy::tasks::Task<DatasetState>>,
}

impl DownloadHandle {
    fn new(scope: DatasetScope, commit_gate: Arc<Mutex<()>>) -> Self {
        Self {
            scope,
            status: Arc::new(Mutex::new(None)),
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            commit_gate,
            #[cfg(not(target_arch = "wasm32"))]
            task: None,
        }
    }
}

/// Runtime ownership for dataset workers, separate from the read-side registry.
#[derive(Resource)]
struct DatasetRuntime {
    operations: HashMap<String, DownloadHandle>,
    #[cfg(not(target_arch = "wasm32"))]
    retiring: Vec<DownloadHandle>,
    commit_gate: Arc<Mutex<()>>,
    #[cfg(not(target_arch = "wasm32"))]
    processors: lunco_assets_processing::process::ProcessorRegistry,
}

impl Default for DatasetRuntime {
    fn default() -> Self {
        Self {
            operations: HashMap::new(),
            #[cfg(not(target_arch = "wasm32"))]
            retiring: Vec::new(),
            commit_gate: Arc::new(Mutex::new(())),
            #[cfg(not(target_arch = "wasm32"))]
            processors: lunco_assets_processing::process::ProcessorRegistry::builtin(),
        }
    }
}

impl DatasetRuntime {
    fn start(&mut self, entry: DatasetEntry, settings: lunco_settings::DownloadSettings) {
        let mut handle = DownloadHandle::new(entry.scope.clone(), self.commit_gate.clone());
        #[cfg(not(target_arch = "wasm32"))]
        {
            handle.task = Some(spawn_download(
                &entry,
                settings,
                handle.status.clone(),
                handle.cancel.clone(),
                handle.commit_gate.clone(),
                self.processors.clone(),
            ));
        }
        #[cfg(target_arch = "wasm32")]
        spawn_download(
            &entry,
            settings,
            handle.status.clone(),
            handle.cancel.clone(),
            handle.commit_gate.clone(),
        );
        self.operations.insert(entry.id, handle);
    }

    fn cancel(&self, id: &str) {
        if let Some(handle) = self.operations.get(id) {
            handle
                .cancel
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }

    /// Stop workers before their scope is removed from the visible registry.
    fn retire_scopes(&mut self, scopes: &[DatasetScope]) {
        if scopes.is_empty() {
            return;
        }
        let _gate = self
            .commit_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let ids: Vec<String> = self
            .operations
            .iter()
            .filter(|(_, handle)| scopes.contains(&handle.scope))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            if let Some(handle) = self.operations.remove(&id) {
                handle
                    .cancel
                    .store(true, std::sync::atomic::Ordering::Release);
                #[cfg(not(target_arch = "wasm32"))]
                self.retiring.push(handle);
            }
        }
    }
}

#[on_command(RequestDataset)]
fn on_request_dataset(
    trigger: On<RequestDataset>,
    mut registry: ResMut<DatasetRegistry>,
    settings: Res<lunco_settings::DownloadSettings>,
    mut runtime: ResMut<DatasetRuntime>,
) {
    let id = &trigger.event().id;
    let Some(entry) = registry.entry(id).cloned() else {
        return;
    };
    if !registry.request(id) {
        return;
    }
    runtime.start(entry, settings.clone());
}

#[on_command(CancelDataset)]
fn on_cancel_dataset(
    trigger: On<CancelDataset>,
    mut registry: ResMut<DatasetRegistry>,
    runtime: Res<DatasetRuntime>,
) {
    if registry.cancel(&trigger.event().id) {
        runtime.cancel(&trigger.event().id);
    }
}

fn on_twin_closed(
    trigger: On<lunco_workspace::TwinClosed>,
    mut registry: ResMut<DatasetRegistry>,
    mut runtime: ResMut<DatasetRuntime>,
    mut commands: Commands,
) {
    let scopes = registry.scopes_for_root(&trigger.event().root);
    runtime.retire_scopes(&scopes);
    for scope in scopes {
        registry.forget_scope(&scope);
        commands.trigger(DatasetScopeRemoved { scope });
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_download(
    entry: &DatasetEntry,
    settings: lunco_settings::DownloadSettings,
    status: StatusSlot,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    commit_gate: Arc<Mutex<()>>,
    processors: lunco_assets_processing::process::ProcessorRegistry,
) -> bevy::tasks::Task<DatasetState> {
    use std::sync::atomic::Ordering;

    use lunco_assets_download::download::{download_asset_with_control, DownloadControl};

    let key = entry.key.clone();
    let spec = entry.spec.clone();
    let scope = entry.scope.clone();
    let destination = entry.path.clone();
    let destination_root = scope.cache_root(spec.shared);
    info!(
        "[datasets] downloading '{}' ({}) — user-requested",
        key, entry.name
    );

    bevy::tasks::AsyncComputeTaskPool::get().spawn(async move {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let process_control = lunco_assets_processing::process::ProcessControl::new(
                cancel.clone(),
                commit_gate.clone(),
            );
            let progress_slot = status.clone();
            let extracting_slot = status.clone();
            let download_control = DownloadControl {
                progress: Some(Box::new(move |done, total| {
                    if let Ok(mut slot) = progress_slot.lock() {
                        *slot = Some(DatasetState::Downloading {
                            bytes_done: done,
                            bytes_total: total,
                        });
                    }
                })),
                extracting: Some(Box::new(move |entries| {
                    if let Ok(mut slot) = extracting_slot.lock() {
                        *slot = Some(DatasetState::Processing {
                            kind: format!("extracting archive ({entries} entries)"),
                        });
                    }
                })),
                cancel: Some(cancel.clone()),
                commit_gate: Some(commit_gate.clone()),
            };
            match download_asset_with_control(
                &spec,
                &key,
                &settings,
                download_control,
                Some(destination_root.as_path()),
            ) {
                Ok(()) => {
                    if let Some(process) = &spec.process {
                        if let Ok(mut slot) = status.lock() {
                            *slot = Some(DatasetState::Processing {
                                kind: process.kind.clone(),
                            });
                        }
                    }
                    match run_process_step(
                        &spec,
                        &scope,
                        &destination,
                        &process_control,
                        &processors,
                    ) {
                        Ok(()) => DatasetState::Installed,
                        Err(error) if cancel.load(Ordering::Acquire) => {
                            let _ = error;
                            DatasetState::Cancelled
                        }
                        Err(error) => DatasetState::Failed(format!("processing failed: {error}")),
                    }
                }
                Err(lunco_assets_download::download::DownloadError::Cancelled) => {
                    DatasetState::Cancelled
                }
                Err(error) => DatasetState::Failed(error.to_string()),
            }
        }))
        .unwrap_or_else(|_| DatasetState::Failed("dataset worker panicked".into()))
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn run_process_step(
    spec: &AssetEntry,
    scope: &DatasetScope,
    destination: &std::path::Path,
    control: &lunco_assets_processing::process::ProcessControl,
    processors: &lunco_assets_processing::process::ProcessorRegistry,
) -> Result<(), std::io::Error> {
    let Some(process) = &spec.process else {
        return Ok(());
    };
    let twin_root = match scope {
        DatasetScope::Twin { root, .. } => Some(root.clone()),
        DatasetScope::Engine => None,
    };
    let cache_root = scope.cache_root(spec.shared);
    info!(
        "[datasets] processing '{}' ({})",
        process.kind,
        destination.display()
    );
    lunco_assets_processing::process::process_asset_with_registry(
        destination,
        process,
        &cache_root,
        twin_root.as_deref(),
        control,
        processors,
    )
}

#[cfg(target_arch = "wasm32")]
fn spawn_download(
    entry: &DatasetEntry,
    _settings: lunco_settings::DownloadSettings,
    status: StatusSlot,
    _cancel: Arc<std::sync::atomic::AtomicBool>,
    _commit_gate: Arc<Mutex<()>>,
) {
    warn!(
        "[datasets] '{}' cannot be downloaded in the browser build — it is served by the host",
        entry.key
    );
    if let Ok(mut slot) = status.lock() {
        *slot = Some(DatasetState::Failed("not downloadable on web".into()));
    }
}

fn apply_state(
    registry: &mut DatasetRegistry,
    id: &str,
    state: DatasetState,
    commands: &mut Commands,
) {
    if !registry.set_state(id, state.clone()) {
        return;
    }
    if let DatasetState::Failed(error) = &state {
        warn!("[datasets] '{id}' failed: {error}");
        commands.trigger(dataset_failed(format!("dataset '{id}' failed: {error}")));
    }
    if state.is_installed() {
        if let Some(entry) = registry.entry(id) {
            info!("[datasets] '{}' installed", entry.key);
            commands.trigger(DatasetInstalled {
                id: entry.id.clone(),
                scope: entry.scope.clone(),
                artifact_path: entry.artifact_path(),
                artifact_uri: entry.artifact_uri(),
            });
        }
    }
}

fn drain_dataset_status(
    mut registry: ResMut<DatasetRegistry>,
    mut runtime: ResMut<DatasetRuntime>,
    mut commands: Commands,
) {
    for detail in registry.take_pending_failures() {
        commands.trigger(dataset_failed(detail));
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        use bevy::tasks::futures_lite::future;
        let mut retiring = Vec::new();
        for mut handle in std::mem::take(&mut runtime.retiring) {
            let finished = handle
                .task
                .as_mut()
                .and_then(|task| future::block_on(future::poll_once(task)))
                .is_some();
            if !finished {
                retiring.push(handle);
            }
        }
        runtime.retiring = retiring;
    }

    let mut updates = Vec::new();
    let mut completed = Vec::new();
    for (id, handle) in &mut runtime.operations {
        let next = match handle.status.lock() {
            Ok(mut slot) => slot.take(),
            Err(_) => Some(DatasetState::Failed(
                "dataset status channel poisoned".into(),
            )),
        };
        if let Some(state) = next {
            #[cfg(target_arch = "wasm32")]
            let terminal = matches!(
                state,
                DatasetState::Installed | DatasetState::Cancelled | DatasetState::Failed(_)
            );
            updates.push((id.clone(), state));
            #[cfg(target_arch = "wasm32")]
            if terminal {
                completed.push(id.clone());
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            use bevy::tasks::futures_lite::future;
            if let Some(task) = handle.task.as_mut() {
                if let Some(state) = future::block_on(future::poll_once(task)) {
                    completed.push(id.clone());
                    updates.push((id.clone(), state));
                }
            }
        }
    }

    for (id, state) in updates {
        apply_state(&mut registry, &id, state, &mut commands);
    }
    for id in completed {
        runtime.operations.remove(&id);
    }
}

/// Installs dataset discovery, state, and explicit provisioning workers.
pub struct DatasetProvisioningPlugin {
    #[cfg(not(target_arch = "wasm32"))]
    processors: lunco_assets_processing::process::ProcessorRegistry,
}

impl Default for DatasetProvisioningPlugin {
    fn default() -> Self {
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            processors: lunco_assets_processing::process::ProcessorRegistry::builtin(),
        }
    }
}

impl DatasetProvisioningPlugin {
    /// Add or replace a native processor before the worker runtime starts.
    ///
    /// The processor is still selected by the authored manifest `kind`; Rhai
    /// composes that policy, while this Rust seam supplies the heavy native
    /// implementation.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_processor(
        mut self,
        processor: lunco_assets_processing::process::ProcessorSpec,
    ) -> Self {
        self.processors.register(processor);
        self
    }
}

impl Plugin for DatasetProvisioningPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(not(target_arch = "wasm32"))]
        app.insert_resource(DatasetRuntime {
            processors: self.processors.clone(),
            ..Default::default()
        });
        #[cfg(target_arch = "wasm32")]
        app.init_resource::<DatasetRuntime>();
        app.init_resource::<lunco_assets_datasets::DatasetProvisioningActive>();
        register_commands!(on_request_dataset, on_cancel_dataset);
        register_all_commands(app);
        app.add_observer(on_twin_closed);
        app.add_systems(Update, drain_dataset_status);
    }
}
