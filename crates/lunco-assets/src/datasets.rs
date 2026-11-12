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
#[cfg(not(target_arch = "wasm32"))]
use lunco_assets_datasets::AssetEntry;
use lunco_assets_datasets::{
    CancelDataset, DatasetEntry, DatasetInstalled, DatasetRegistry, DatasetScope,
    DatasetScopeRemoved, DatasetState, ProcessDataset, RequestDataset, dataset_failed,
    dataset_state_changed,
};
use lunco_command_contracts::OpId;
use lunco_core::{on_command, register_commands};

/// Publish one phase to the diagnostic stream and the optional desktop status
/// consumer. Errors already reach status through the shared telemetry bridge.
fn report_phase(commands: &mut Commands, id: &str, name: &str, state: &DatasetState) {
    let event = dataset_state_changed(id, name, state);
    if !matches!(state, DatasetState::Failed(_)) {
        let phase = match state {
            DatasetState::Missing => "Not downloaded".to_owned(),
            DatasetState::Queued => "Queued; waiting for a preparation slot".to_owned(),
            DatasetState::Downloading { source, .. } => format!("Downloading source: {source}"),
            DatasetState::Processing { kind } => format!("Preparing: {kind}"),
            DatasetState::Cancelling => "Stopping preparation".to_owned(),
            DatasetState::Installed => "Data ready".to_owned(),
            DatasetState::Cancelled => {
                "Cancelled; available to resume in Settings > Data & libraries".to_owned()
            }
            DatasetState::Failed(_) => unreachable!(),
        };
        let message = format!("{name}: {phase}");
        commands.queue(move |world: &mut World| {
            if let Some(mut status) =
                world.get_resource_mut::<lunco_status_core::status_bus::StatusBus>()
            {
                status.push(
                    "datasets",
                    lunco_status_core::status_bus::StatusLevel::Info,
                    message,
                );
            }
        });
    }
    commands.trigger(event);
}

/// Cross-thread slot for progress produced by one download worker.
#[derive(Default)]
struct DatasetProgress {
    state: Option<DatasetState>,
    inputs: Option<Vec<lunco_assets_datasets::DatasetInput>>,
}
type StatusSlot = Arc<Mutex<DatasetProgress>>;

/// One owned provisioning operation.
struct DatasetOperation {
    scope: DatasetScope,
    status: StatusSlot,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    commit_gate: Arc<Mutex<()>>,
    #[cfg(not(target_arch = "wasm32"))]
    task: Option<bevy::tasks::Task<DatasetState>>,
}

impl DatasetOperation {
    fn new(scope: DatasetScope, commit_gate: Arc<Mutex<()>>) -> Self {
        Self {
            scope,
            status: Arc::new(Mutex::new(DatasetProgress::default())),
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
    operations: HashMap<String, DatasetOperation>,
    pending: std::collections::VecDeque<(DatasetEntry, PendingOperation)>,
    #[cfg(not(target_arch = "wasm32"))]
    retiring: Vec<DatasetOperation>,
    commit_gate: Arc<Mutex<()>>,
    #[cfg(not(target_arch = "wasm32"))]
    processors: lunco_assets_processing::process::ProcessorRegistry,
}

impl Default for DatasetRuntime {
    fn default() -> Self {
        Self {
            operations: HashMap::new(),
            pending: Default::default(),
            #[cfg(not(target_arch = "wasm32"))]
            retiring: Vec::new(),
            commit_gate: Arc::new(Mutex::new(())),
            #[cfg(not(target_arch = "wasm32"))]
            processors: lunco_assets_processing::process::ProcessorRegistry::builtin(),
        }
    }
}

enum PendingOperation {
    Download,
    #[cfg(not(target_arch = "wasm32"))]
    Process {
        force: bool,
    },
}

impl DatasetRuntime {
    fn start(
        &mut self,
        entry: DatasetEntry,
        settings: lunco_settings::DownloadSettings,
        decode_budget: u64,
    ) {
        let handle = DatasetOperation::new(entry.scope.clone(), self.commit_gate.clone());
        #[cfg(not(target_arch = "wasm32"))]
        let handle = {
            let mut handle = handle;
            handle.task = Some(spawn_download(
                &entry,
                settings,
                handle.status.clone(),
                handle.cancel.clone(),
                handle.commit_gate.clone(),
                self.processors.clone(),
                decode_budget,
            ));
            handle
        };
        #[cfg(target_arch = "wasm32")]
        let handle = {
            spawn_download(
                &entry,
                settings,
                handle.status.clone(),
                handle.cancel.clone(),
                handle.commit_gate.clone(),
            );
            handle
        };
        self.operations.insert(entry.id, handle);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn start_processing(&mut self, entry: DatasetEntry, force: bool, decode_budget: u64) {
        let mut handle = DatasetOperation::new(entry.scope.clone(), self.commit_gate.clone());
        handle.task = Some(spawn_processing(
            &entry,
            handle.cancel.clone(),
            handle.commit_gate.clone(),
            self.processors.clone(),
            handle.status.clone(),
            force,
            decode_budget,
        ));
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
        self.pending
            .retain(|(entry, _)| !scopes.contains(&entry.scope));
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
    mut runtime: ResMut<DatasetRuntime>,
    mut commands: Commands,
) -> Result<lunco_command_contracts::Ack, String> {
    let id = &trigger.event().id;
    let entry = registry
        .entry(id)
        .cloned()
        .ok_or_else(|| format!("dataset is not declared: {id}"))?;
    if !registry.request(id) {
        return Err(format!(
            "dataset '{id}' is already installed or has an active operation"
        ));
    }
    registry.set_state(id, DatasetState::Queued);
    report_phase(&mut commands, id, &entry.name, &DatasetState::Queued);
    runtime
        .pending
        .push_back((entry, PendingOperation::Download));
    Ok(lunco_command_contracts::Ack::new(OpId::new()))
}

#[cfg(not(target_arch = "wasm32"))]
#[on_command(ProcessDataset)]
fn on_process_dataset(
    trigger: On<ProcessDataset>,
    mut registry: ResMut<DatasetRegistry>,
    mut runtime: ResMut<DatasetRuntime>,
    mut commands: Commands,
) -> Result<lunco_command_contracts::Ack, String> {
    let id = &trigger.event().id;
    let entry = registry
        .entry(id)
        .cloned()
        .ok_or_else(|| format!("dataset is not declared: {id}"))?;
    let process = entry
        .spec
        .process
        .as_ref()
        .ok_or_else(|| format!("dataset '{id}' has no manifest processing pipeline"))?;
    if !registry.process(id) {
        return Err(format!("dataset '{id}' already has an active operation"));
    }
    let _ = process;
    registry.set_state(id, DatasetState::Queued);
    report_phase(&mut commands, id, &entry.name, &DatasetState::Queued);
    runtime.pending.push_back((
        entry,
        PendingOperation::Process {
            force: trigger.event().force,
        },
    ));
    Ok(lunco_command_contracts::Ack::new(OpId::new()))
}

#[cfg(target_arch = "wasm32")]
#[on_command(ProcessDataset)]
fn on_process_dataset(
    _trigger: On<ProcessDataset>,
) -> Result<lunco_command_contracts::Ack, String> {
    Err("manifest processing is available only in native builds".into())
}

#[on_command(CancelDataset)]
fn on_cancel_dataset(
    trigger: On<CancelDataset>,
    mut registry: ResMut<DatasetRegistry>,
    mut runtime: ResMut<DatasetRuntime>,
    mut commands: Commands,
) -> Result<lunco_command_contracts::Ack, String> {
    let id = &trigger.event().id;
    let name = registry
        .entry(id)
        .map(|entry| entry.name.clone())
        .ok_or_else(|| format!("dataset is not declared: {id}"))?;
    if !registry.cancel(id) {
        return Err(format!("dataset '{id}' has no cancellable operation"));
    }
    if runtime.pending.iter().any(|(entry, _)| &entry.id == id) {
        runtime.pending.retain(|(entry, _)| &entry.id != id);
        registry.set_state(id, DatasetState::Cancelled);
        report_phase(&mut commands, id, &name, &DatasetState::Cancelled);
    } else {
        runtime.cancel(id);
        report_phase(&mut commands, id, &name, &DatasetState::Cancelling);
    }
    Ok(lunco_command_contracts::Ack::new(OpId::new()))
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

fn on_retire_dataset_scope(
    event: On<lunco_assets_datasets::RetireDatasetScope>,
    mut registry: ResMut<DatasetRegistry>,
    mut runtime: ResMut<DatasetRuntime>,
    mut commands: Commands,
) {
    runtime.retire_scopes(std::slice::from_ref(&event.scope));
    registry.forget_scope(&event.scope);
    commands.trigger(DatasetScopeRemoved {
        scope: event.scope.clone(),
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_download(
    entry: &DatasetEntry,
    settings: lunco_settings::DownloadSettings,
    status: StatusSlot,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    commit_gate: Arc<Mutex<()>>,
    processors: lunco_assets_processing::process::ProcessorRegistry,
    decode_budget: u64,
) -> bevy::tasks::Task<DatasetState> {
    use std::sync::atomic::Ordering;

    use lunco_assets_download::download::{DownloadControl, download_asset_with_control};

    let owned_entry = entry.clone();
    let key = entry.key.clone();
    let spec = entry.spec.clone();
    let scope = entry.scope.clone();
    let destination_root = scope.local_cache_root();
    info!(
        "[datasets] downloading '{}' ({}) — user-requested",
        key, entry.name
    );

    bevy::tasks::AsyncComputeTaskPool::get().spawn(async move {
        let completion_status = status.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let step_status = status.clone();
            let process_control = lunco_assets_processing::process::ProcessControl::new(
                cancel.clone(),
                commit_gate.clone(),
            )
            .with_texture_decode_budget(decode_budget)
            .with_progress(move |step| {
                if let Ok(mut slot) = step_status.lock() {
                    slot.state = Some(DatasetState::Processing {
                        kind: step.to_owned(),
                    });
                }
            });
            let progress_slot = status.clone();
            let extracting_slot = status.clone();
            let completed_slot = status.clone();
            let mut completed_inputs = owned_entry.inputs.clone();
            let source_key = key.clone();
            let download_control = DownloadControl {
                progress: Some(Box::new(move |source, done, total| {
                    if let Ok(mut slot) = progress_slot.lock() {
                        slot.state = Some(DatasetState::Downloading {
                            source: source.to_owned(),
                            bytes_done: done,
                            bytes_total: total,
                        });
                    }
                })),
                completed: Some(Box::new(move |key| {
                    let id = if key == source_key {
                        Some("source")
                    } else {
                        key.strip_prefix(source_key.as_str())
                            .and_then(|suffix| suffix.strip_prefix('/'))
                    };
                    if let Some(input) = completed_inputs
                        .iter_mut()
                        .find(|input| Some(input.id.as_str()) == id)
                    {
                        input.state = lunco_assets_datasets::DatasetInputState::Ready;
                        if let Ok(mut slot) = completed_slot.lock() {
                            slot.inputs = Some(completed_inputs.clone());
                        }
                    }
                })),
                extracting: Some(Box::new(move |entries| {
                    if let Ok(mut slot) = extracting_slot.lock() {
                        slot.state = Some(DatasetState::Processing {
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
                    if let Err(error) = publish_inputs(&owned_entry, &status) {
                        return DatasetState::Failed(error);
                    }
                    if let Some(process) = &spec.process {
                        if let Ok(mut slot) = status.lock() {
                            slot.state = Some(DatasetState::Processing {
                                kind: process.kind.clone(),
                            });
                        }
                    }
                    let processed = if spec.process.is_some() {
                        source_path(&owned_entry).and_then(|source| {
                            run_process_step(&spec, &scope, &source, &process_control, &processors)
                        })
                    } else {
                        Ok(())
                    };
                    match processed.and_then(|()| verify_delivery(&owned_entry)) {
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
        .unwrap_or_else(|_| DatasetState::Failed("dataset worker panicked".into()));
        match publish_inputs(&owned_entry, &completion_status) {
            Ok(()) => result,
            Err(error) => DatasetState::Failed(error),
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_processing(
    entry: &DatasetEntry,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    commit_gate: Arc<Mutex<()>>,
    processors: lunco_assets_processing::process::ProcessorRegistry,
    status: StatusSlot,
    force: bool,
    decode_budget: u64,
) -> bevy::tasks::Task<DatasetState> {
    let owned_entry = entry.clone();
    let spec = entry.spec.clone();
    let scope = entry.scope.clone();
    bevy::tasks::AsyncComputeTaskPool::get().spawn(async move {
        let completion_status = status.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let control =
                lunco_assets_processing::process::ProcessControl::new(cancel.clone(), commit_gate)
                    .with_force(force)
                    .with_texture_decode_budget(decode_budget)
                    .with_progress(move |step| {
                        if let Ok(mut slot) = status.lock() {
                            slot.state = Some(DatasetState::Processing {
                                kind: step.to_owned(),
                            });
                        }
                    });
            match source_path(&owned_entry)
                .and_then(|source| run_process_step(&spec, &scope, &source, &control, &processors))
                .and_then(|()| verify_delivery(&owned_entry))
            {
                Ok(()) => DatasetState::Installed,
                Err(error) if cancel.load(std::sync::atomic::Ordering::Acquire) => {
                    let _ = error;
                    DatasetState::Cancelled
                }
                Err(error) => DatasetState::Failed(format!("processing failed: {error}")),
            }
        }))
        .unwrap_or_else(|_| DatasetState::Failed("dataset worker panicked".into()));
        match publish_inputs(&owned_entry, &completion_status) {
            Ok(()) => result,
            Err(error) => DatasetState::Failed(error),
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn source_path(entry: &DatasetEntry) -> std::io::Result<std::path::PathBuf> {
    lunco_assets_datasets::source_read_path(&entry.spec, entry.read_roots())
}

#[cfg(not(target_arch = "wasm32"))]
fn verify_delivery(entry: &DatasetEntry) -> std::io::Result<()> {
    entry
        .installed_artifact()
        .map_err(std::io::Error::other)?
        .map(|_| ())
        .ok_or_else(|| std::io::Error::other(entry.delivery_error()))
}

#[cfg(not(target_arch = "wasm32"))]
fn publish_inputs(entry: &DatasetEntry, status: &StatusSlot) -> Result<(), String> {
    let inputs = entry.inspect_inputs();
    status
        .lock()
        .map_err(|_| "dataset status channel poisoned".to_owned())?
        .inputs = Some(inputs);
    Ok(())
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
        slot.state = Some(DatasetState::Failed("not downloadable on web".into()));
    }
}

fn apply_state(
    registry: &mut DatasetRegistry,
    id: &str,
    state: DatasetState,
    commands: &mut Commands,
) {
    let changed_phase = registry.state(id).is_some_and(|previous| {
        previous.kind() != state.kind()
            || matches!((previous, &state), (DatasetState::Downloading { source: previous, .. }, DatasetState::Downloading { source: current, .. }) if previous != current)
            || (matches!(state, DatasetState::Processing { .. } | DatasetState::Failed(_)) && previous != &state)
    });
    if !registry.set_state(id, state.clone()) {
        return;
    }
    if let DatasetState::Failed(error) = &state {
        warn!("[datasets] '{id}' failed: {error}");
    }
    if changed_phase {
        if let Some(entry) = registry.entry(id) {
            report_phase(commands, id, &entry.name, &state);
        }
    }
    if state.is_installed() {
        if let Some(entry) = registry.entry(id) {
            info!("[datasets] '{}' installed", entry.key);
            commands.trigger(DatasetInstalled {
                id: entry.id.clone(),
                scope: entry.scope.clone(),
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
    // Failure delivery leaves dataset catalog change ticks to its producers.
    for detail in registry.bypass_change_detection().take_pending_failures() {
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
            Ok(mut slot) => {
                if let Some(inputs) = slot.inputs.take() {
                    registry.set_inputs(id, inputs);
                }
                slot.state.take()
            }
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

fn admit_dataset_operations(
    settings: Res<lunco_settings::DownloadSettings>,
    processing_settings: Res<lunco_settings::AssetProcessingSettings>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut runtime: ResMut<DatasetRuntime>,
    mut registry: ResMut<DatasetRegistry>,
    mut commands: Commands,
) {
    loop {
        let active = runtime.operations.len();
        #[cfg(not(target_arch = "wasm32"))]
        let active = active + runtime.retiring.len();
        if active >= settings.max_parallel_downloads.max(1) {
            break;
        }
        let Some((entry, operation)) = runtime.pending.pop_front() else {
            break;
        };
        if registry.state(&entry.id) != Some(&DatasetState::Queued) {
            continue;
        }
        let decode_budget = (|| {
            let manifest = match &entry.scope {
                DatasetScope::Engine => None,
                DatasetScope::Twin { root, .. } => {
                    let twin = workspace
                        .as_ref()
                        .and_then(|w| {
                            w.twins()
                                .find(|(_, twin)| &twin.root == root)
                                .map(|(_, twin)| twin)
                        })
                        .ok_or_else(|| {
                            format!("dataset '{}' has no admitted owning Twin", entry.id)
                        })?;
                    twin.manifest.as_ref()
                }
            };
            crate::settings::texture_decode_budget(
                &processing_settings,
                manifest.and_then(|m| m.setting(crate::settings::TWIN_TEXTURE_DECODE_MAX_BYTES)),
            )
        })();
        let decode_budget = match decode_budget {
            Ok(bytes) => bytes,
            Err(error) => {
                apply_state(
                    &mut registry,
                    &entry.id,
                    DatasetState::Failed(error),
                    &mut commands,
                );
                continue;
            }
        };
        let state = match operation {
            PendingOperation::Download => DatasetState::Downloading {
                source: entry.name.clone(),
                bytes_done: 0,
                bytes_total: 0,
            },
            #[cfg(not(target_arch = "wasm32"))]
            PendingOperation::Process { .. } => DatasetState::Processing {
                kind: entry
                    .spec
                    .process
                    .as_ref()
                    .expect("admitted process declaration")
                    .kind
                    .clone(),
            },
        };
        apply_state(&mut registry, &entry.id, state, &mut commands);
        match operation {
            PendingOperation::Download => runtime.start(entry, settings.clone(), decode_budget),
            #[cfg(not(target_arch = "wasm32"))]
            PendingOperation::Process { force } => {
                runtime.start_processing(entry, force, decode_budget)
            }
        }
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
        use lunco_settings::AppSettingsExt;
        app.register_settings_section::<lunco_settings::AssetProcessingSettings>();
        #[cfg(not(target_arch = "wasm32"))]
        app.insert_resource(DatasetRuntime {
            processors: self.processors.clone(),
            ..Default::default()
        });
        #[cfg(target_arch = "wasm32")]
        app.init_resource::<DatasetRuntime>();
        app.init_resource::<lunco_assets_datasets::DatasetProvisioningActive>();
        register_commands!(on_request_dataset, on_process_dataset, on_cancel_dataset);
        register_all_commands(app);
        app.add_observer(on_twin_closed);
        app.add_observer(on_retire_dataset_scope);
        app.add_systems(
            Update,
            (drain_dataset_status, admit_dataset_operations).chain(),
        );
    }
}
