//! Native source-library provisioning and editor-index lifecycle.
//!
//! The compiler/runtime host installs an already materialised source root and exposes
//! the generic load state. This application adapter composes dataset delivery
//! with the Modelica asset indexer and keeps the blocking scan/decode off the
//! render thread.

use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use lunco_assets_datasets::{DatasetRegistry, DatasetState};
use lunco_assets_runtime::library::{LibraryLoadPhase, LibraryLoadState, LibrarySource};
use lunco_modelica_index::visual_diagram::LIBRARY_INDEX_FILE_NAME;
use lunco_modelica_source_roots::NativeLibraryIndexLoad;

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
            (drain_native_library_install, drive_native_library_dataset).chain(),
        );

        let Some(root) = lunco_assets_runtime::library::primary_filesystem_library_root()
            .map(std::path::Path::to_path_buf)
        else {
            return;
        };

        app.init_resource::<NativeLibraryIndexLoad>();
        if !root.join(LIBRARY_INDEX_FILE_NAME).is_file() {
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
                )
            }));

            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }

            let outcome = completed
                .map_err(|_| "source library indexer panicked".to_owned())
                .and_then(|result| result);
            if let Err(error) = outcome {
                set_install_state(&slot, LibraryLoadState::Failed(error));
                return;
            }
            if !root.join(LIBRARY_INDEX_FILE_NAME).is_file() {
                set_install_state(
                    &slot,
                    LibraryLoadState::Failed(format!(
                        "source library editor index was not generated at {}",
                        root.join(LIBRARY_INDEX_FILE_NAME).display()
                    )),
                );
                return;
            }

            set_install_state(
                &slot,
                LibraryLoadState::Ready {
                    file_count: lunco_assets_runtime::library::filesystem_library_file_count(),
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
    let root =
        match lunco_modelica_library::source_library::configured_native_library_root(&settings) {
            Ok(Some(root)) => root,
            Ok(None) => {
                commands.insert_resource(LibraryLoadState::Failed(
                    "no configured source library is installed".into(),
                ));
                return;
            }
            Err(error) => {
                commands.insert_resource(LibraryLoadState::Failed(error.to_string()));
                return;
            }
        };
    if let Some(existing) = existing {
        existing
            .cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    if let Err(error) = lunco_assets_runtime::library::install_global_library_sources(vec![
        LibrarySource::Filesystem(root.clone()),
    ]) {
        commands.insert_resource(LibraryLoadState::Failed(error.to_string()));
        return;
    }
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
    settings: Res<lunco_modelica_library::LibrarySettings>,
    mut commands: Commands,
) {
    if settings.local_root_override.is_some() {
        return;
    }
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
            let root = match lunco_modelica_library::source_library::source_library_root_path() {
                Ok(Some(root)) => root,
                Ok(None) => {
                    *state = LibraryLoadState::Failed(
                        "dataset is installed but no source tree exists".into(),
                    );
                    return;
                }
                Err(error) => {
                    *state = LibraryLoadState::Failed(error.to_string());
                    return;
                }
            };
            if let Err(error) = lunco_assets_runtime::library::install_global_library_sources(vec![
                LibrarySource::Filesystem(root.clone()),
            ]) {
                *state = LibraryLoadState::Failed(error.to_string());
                return;
            }
            if root.join(LIBRARY_INDEX_FILE_NAME).is_file() {
                *state = LibraryLoadState::Ready {
                    file_count: lunco_assets_runtime::library::filesystem_library_file_count(),
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
