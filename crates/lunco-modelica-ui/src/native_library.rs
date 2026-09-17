//! Native source-library provisioning and editor-index lifecycle.
//!
//! The compiler core installs an already materialised source root and exposes
//! the generic load state. This application adapter composes dataset delivery
//! with the Modelica asset indexer and keeps the blocking scan/decode off the
//! render thread.

use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use lunco_assets_core::library::{LibraryLoadPhase, LibraryLoadState, LibrarySource};
use lunco_assets_datasets::{DatasetRegistry, DatasetState};

const NATIVE_LIBRARY_DATASET_ID: &str = "engine/modelica/library";

type NativeInstallSlot = Arc<Mutex<NativeInstallSlotInner>>;

#[derive(Default)]
struct NativeInstallSlotInner {
    pending_state: Option<LibraryLoadState>,
}

#[derive(Resource)]
struct NativeLibraryInstallSlot {
    state: NativeInstallSlot,
    cancel: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Resource, Default)]
struct NativeLibraryIndexLoad {
    task: Option<
        bevy::tasks::Task<Result<lunco_modelica_index::visual_diagram::LibraryIndex, String>>,
    >,
    failed: bool,
}

impl NativeLibraryIndexLoad {
    fn new() -> Self {
        Self {
            task: None,
            failed: false,
        }
    }
}

/// User intent to rebuild the native editor index after a failed indexing or
/// index-load attempt. Dataset downloading remains generic.
#[derive(Event, Clone, Copy, Debug)]
pub(crate) enum NativeLibraryIndexAction {
    /// Re-scan the configured source root.
    Rebuild,
}

/// Composes native source-library provisioning with the Modelica workbench.
pub(crate) struct NativeLibraryIndexerPlugin;

impl Plugin for NativeLibraryIndexerPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_native_library_index_action);
        app.add_systems(
            Update,
            (
                drain_native_library_install,
                drive_native_library_dataset,
                drive_native_library_index,
            )
                .chain(),
        );

        let settings = app
            .world()
            .resource::<lunco_modelica_library::LibrarySettings>();
        let Some(root) =
            lunco_modelica_library::source_library::configured_native_library_root(settings)
        else {
            return;
        };

        app.init_resource::<NativeLibraryIndexLoad>();
        if !root.join("library_index.json").is_file() {
            info!(
                "[source library] source root is present but its generated editor index is missing; indexing in the background"
            );
            app.insert_resource(LibraryLoadState::Loading {
                phase: LibraryLoadPhase::Parsing,
                bytes_done: 0,
                bytes_total: 0,
            });
            app.insert_resource(start_native_index(root));
        }
    }
}

fn start_native_index(root: std::path::PathBuf) -> NativeLibraryInstallSlot {
    let slot: NativeInstallSlot = Arc::new(Mutex::new(NativeInstallSlotInner::default()));
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    spawn_native_index(slot.clone(), root, cancel.clone());
    NativeLibraryInstallSlot {
        state: slot,
        cancel,
    }
}

fn spawn_native_index(
    slot: NativeInstallSlot,
    root: std::path::PathBuf,
    cancel: Arc<std::sync::atomic::AtomicBool>,
) {
    bevy::tasks::AsyncComputeTaskPool::get()
        .spawn(async move {
            info!(
                "[source library] indexing editor metadata for {}…",
                root.display()
            );
            let completed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                lunco_modelica_assets::indexer::run_with_cancel(
                    lunco_modelica_assets::indexer::Options::for_source_root(root.clone()),
                    Some(cancel.clone()),
                );
            }))
            .is_ok();

            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }

            if !completed || !root.join("library_index.json").is_file() {
                set_install_state(
                    &slot,
                    LibraryLoadState::Failed(format!(
                        "source library editor index was not generated at {}",
                        root.join("library_index.json").display()
                    )),
                );
                return;
            }

            set_install_state(
                &slot,
                LibraryLoadState::Ready {
                    file_count: lunco_assets_core::library::filesystem_library_file_count(),
                    compressed_bytes: 0,
                    uncompressed_bytes: 0,
                },
            );
        })
        .detach();
}

fn set_install_state(slot: &NativeInstallSlot, state: LibraryLoadState) {
    if let Ok(mut inner) = slot.lock() {
        inner.pending_state = Some(state);
    }
}

fn drain_native_library_install(
    slot: Option<Res<NativeLibraryInstallSlot>>,
    mut state: ResMut<LibraryLoadState>,
) {
    let Some(slot) = slot else { return };
    let Ok(mut inner) = slot.state.lock() else {
        return;
    };
    if let Some(new_state) = inner.pending_state.take() {
        match (&*state, &new_state) {
            (
                LibraryLoadState::Loading { phase: a, .. },
                LibraryLoadState::Loading { phase: b, .. },
            ) if a == b => {}
            _ => lunco_modelica_library::source_library::log_library_state_transition(&new_state),
        }
        *state = new_state;
    }
}

fn on_native_library_index_action(
    trigger: On<NativeLibraryIndexAction>,
    state: Res<LibraryLoadState>,
    settings: Res<lunco_modelica_library::LibrarySettings>,
    existing: Option<Res<NativeLibraryInstallSlot>>,
    mut commands: Commands,
) {
    if !matches!(*trigger.event(), NativeLibraryIndexAction::Rebuild)
        || !matches!(*state, LibraryLoadState::Failed(_))
    {
        return;
    }
    let Some(root) =
        lunco_modelica_library::source_library::configured_native_library_root(&settings)
    else {
        return;
    };
    if let Some(existing) = existing {
        existing
            .cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    lunco_assets_core::library::install_global_library_sources(vec![LibrarySource::Filesystem(
        root.clone(),
    )]);
    commands.insert_resource(LibraryLoadState::Loading {
        phase: LibraryLoadPhase::Parsing,
        bytes_done: 0,
        bytes_total: 0,
    });
    commands.insert_resource(NativeLibraryIndexLoad::new());
    commands.insert_resource(start_native_index(root));
}

fn drive_native_library_dataset(
    registry: Option<Res<DatasetRegistry>>,
    slot: Option<Res<NativeLibraryInstallSlot>>,
    index_load: Option<Res<NativeLibraryIndexLoad>>,
    mut state: ResMut<LibraryLoadState>,
    mut commands: Commands,
) {
    let Some(registry) = registry else { return };
    // A configured source root is already owned by the Modelica lifecycle
    // below. The generic dataset state must not overwrite its ready/indexing
    // state when a user supplied a local root or the cache was discovered
    // before dataset manifests were scanned.
    if slot.is_some() || index_load.is_some() {
        return;
    }
    let Some(dataset) = registry
        .entries()
        .iter()
        .find(|entry| entry.id == NATIVE_LIBRARY_DATASET_ID)
    else {
        return;
    };

    match &dataset.state {
        DatasetState::Missing => *state = LibraryLoadState::NotStarted,
        DatasetState::Downloading {
            bytes_done,
            bytes_total,
        } => {
            *state = LibraryLoadState::Loading {
                phase: LibraryLoadPhase::FetchingBundle,
                bytes_done: *bytes_done,
                bytes_total: *bytes_total,
            };
        }
        DatasetState::Processing { .. } => {
            *state = LibraryLoadState::Loading {
                phase: LibraryLoadPhase::Parsing,
                bytes_done: 0,
                bytes_total: 0,
            };
        }
        DatasetState::Cancelling => {
            *state = LibraryLoadState::Loading {
                phase: LibraryLoadPhase::FetchingBundle,
                bytes_done: 0,
                bytes_total: 0,
            };
        }
        DatasetState::Cancelled => *state = LibraryLoadState::NotStarted,
        DatasetState::Failed(error) => *state = LibraryLoadState::Failed(error.clone()),
        DatasetState::Installed => {
            let Some(root) = lunco_assets_core::source_library_root_path("library") else {
                *state = LibraryLoadState::Failed(
                    "dataset is installed but no Modelica source tree exists in the cache".into(),
                );
                return;
            };
            lunco_assets_core::library::install_global_library_sources(vec![
                LibrarySource::Filesystem(root.clone()),
            ]);
            if root.join("library_index.json").is_file() {
                *state = LibraryLoadState::Ready {
                    file_count: lunco_assets_core::library::filesystem_library_file_count(),
                    compressed_bytes: 0,
                    uncompressed_bytes: 0,
                };
                if index_load.is_none() {
                    commands.insert_resource(NativeLibraryIndexLoad::new());
                }
                return;
            }
            *state = LibraryLoadState::Loading {
                phase: LibraryLoadPhase::Parsing,
                bytes_done: 0,
                bytes_total: 0,
            };
            commands.insert_resource(NativeLibraryIndexLoad::new());
            commands.insert_resource(start_native_index(root));
        }
    }
}

fn drive_native_library_index(
    index_load: Option<ResMut<NativeLibraryIndexLoad>>,
    state: Option<ResMut<LibraryLoadState>>,
    mut commands: Commands,
) {
    use bevy::tasks::futures_lite::future;

    let Some(mut index_load) = index_load else {
        return;
    };

    if index_load.failed || lunco_modelica_index::visual_diagram::library_index_available() {
        return;
    }

    if index_load.task.is_some() {
        let result = {
            let task = index_load
                .task
                .as_mut()
                .expect("source library index task disappeared while being polled");
            future::block_on(future::poll_once(task))
        };
        let Some(result) = result else {
            return;
        };
        index_load.task = None;
        match result {
            Ok(index) => {
                if lunco_modelica_index::visual_diagram::install_library_index(index) {
                    info!("[source library] editor index loaded off-thread");
                    commands.trigger(
                        lunco_modelica_index::visual_diagram::LibraryEditorIndexBecameReady,
                    );
                }
            }
            Err(error) => {
                index_load.failed = true;
                error!("[source library] editor index load failed: {error}");
                if let Some(mut state) = state {
                    *state = LibraryLoadState::Failed(error);
                }
            }
        }
        return;
    }

    if !state.is_some_and(|state| state.is_ready()) {
        return;
    }

    info!("[source library] loading editor index off-thread");
    index_load.task =
        Some(bevy::tasks::AsyncComputeTaskPool::get().spawn(async {
            lunco_modelica_index::visual_diagram::load_library_index_from_assets()
        }));
}
