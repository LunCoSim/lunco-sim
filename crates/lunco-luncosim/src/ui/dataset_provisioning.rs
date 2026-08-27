//! Interactive consent for declared datasets.
//!
//! The asset registry owns identity, storage, downloading, processing, and
//! cancellation. This module owns only the windowed presentation and invokes
//! the authored provisioning policy with registry facts. A button dispatches
//! the same typed dataset command used by every other UI and script. The
//! unchecked negative popup option is persisted in the active Twin's manifest.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_assets::datasets::{
    DatasetEntry, DatasetScope, DatasetScopeReady, DatasetScopeRemoved, DatasetState,
    RequestDataset,
};
use lunco_ui::modal::{ModalBody, ModalButton, ModalId, ModalOutcome, ModalQueue, ModalRequest};
use lunco_workspace::WorkspaceResource;

#[derive(Resource, Default)]
pub(crate) struct DatasetProvisioningState {
    pending: Vec<ProvisioningRequest>,
    active: Option<ActiveProvisioning>,
}

struct ProvisioningRequest {
    scope: DatasetScope,
    datasets: Vec<ProvisionedDataset>,
    selection: Arc<std::sync::Mutex<Vec<bool>>>,
    project_root: Option<PathBuf>,
    suppress_prompt: Arc<std::sync::Mutex<bool>>,
}

struct ActiveProvisioning {
    request: ProvisioningRequest,
    modal: ModalId,
}

#[derive(Clone)]
struct ProvisionedDataset {
    id: String,
    name: String,
    state: DatasetState,
    has_processing: bool,
}

fn visible_entries<'a>(
    registry: &'a lunco_assets::datasets::DatasetRegistry,
    scope: &DatasetScope,
) -> Vec<&'a DatasetEntry> {
    registry
        .entries()
        .iter()
        .filter(|entry| &entry.scope == scope)
        // Startup consent is onboarding, not an inventory of every optional
        // resource a Twin may declare. The manifest's `recommended` bit is the
        // authoritative dependency boundary for this prompt; the resource
        // manager remains the place to request non-recommended candidates.
        .filter(|entry| entry.recommended)
        .collect()
}

fn needs_provisioning(state: &DatasetState) -> bool {
    matches!(
        state,
        DatasetState::Missing | DatasetState::Failed(_) | DatasetState::Cancelled
    )
}

fn same_scope(state: &DatasetProvisioningState, scope: &DatasetScope) -> bool {
    state.pending.iter().any(|request| &request.scope == scope)
        || state
            .active
            .as_ref()
            .is_some_and(|active| &active.request.scope == scope)
}

fn open_modal(request: ProvisioningRequest, modals: &mut ModalQueue) -> ActiveProvisioning {
    let is_engine = matches!(request.scope, DatasetScope::Engine);
    let owner = request.scope.label().to_owned();
    let body_choices = request.datasets.clone();
    let selection = request.selection.clone();
    let project_root = request.project_root.clone();
    let suppress_prompt = request.suppress_prompt.clone();
    let body = ModalBody::Custom(Arc::new(move |ui| {
        if is_engine {
            ui.label("Choose which recommended visual resources to download.");
        } else {
            ui.label(format!("Choose data to download for Twin '{owner}'."));
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("Select all missing").clicked() {
                if let Ok(mut selected) = selection.lock() {
                    for (value, dataset) in selected.iter_mut().zip(&body_choices) {
                        *value = needs_provisioning(&dataset.state);
                    }
                }
            }
            if ui.button("Clear selection").clicked() {
                if let Ok(mut selected) = selection.lock() {
                    selected.fill(false);
                }
            }
        });
        // Keep the consent controls and action row reachable on small displays:
        // only the potentially unbounded dataset list scrolls.
        let list_height = (ui.ctx().content_rect().height() - 190.0).max(32.0);
        egui::ScrollArea::vertical()
            .max_height(list_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (index, dataset) in body_choices.iter().enumerate() {
                    let Ok(mut selected) = selection.lock() else {
                        return;
                    };
                    let enabled = needs_provisioning(&dataset.state);
                    let mut checked = selected[index];
                    ui.horizontal(|ui| {
                        ui.add_enabled(enabled, egui::Checkbox::new(&mut checked, &dataset.name));
                        ui.label(status_text(dataset));
                    });
                    selected[index] = checked;
                }
            });
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(
                "Already ready resources are shown for visibility and cannot be selected.",
            )
            .weak()
            .small(),
        );
        if project_root.is_some() {
            ui.separator();
            let Ok(mut suppress) = suppress_prompt.lock() else {
                return;
            };
            ui.checkbox(&mut *suppress, "Don't ask again for this project");
        }
    }));
    let title = if is_engine {
        "Download visual resources"
    } else {
        "Twin data is not installed"
    };
    let modal = modals.request(ModalRequest {
        title: title.into(),
        body,
        buttons: vec![
            ModalButton::Confirm("Download selected".into()),
            ModalButton::Cancel("Not now".into()),
        ],
        dismiss_on_esc: true,
    });
    ActiveProvisioning { request, modal }
}

fn status_text(dataset: &ProvisionedDataset) -> String {
    match &dataset.state {
        DatasetState::Installed => "Ready".into(),
        DatasetState::Missing => {
            if dataset.has_processing {
                "Not downloaded · prepares locally".into()
            } else {
                "Not downloaded".into()
            }
        }
        DatasetState::Downloading { .. } => "Downloading…".into(),
        DatasetState::Processing { kind } => format!("Preparing ({kind})…"),
        DatasetState::Cancelling => "Stopping…".into(),
        DatasetState::Cancelled => "Cancelled".into(),
        DatasetState::Failed(error) => format!("Failed: {error}"),
    }
}

fn show_next(state: &mut DatasetProvisioningState, modals: &mut ModalQueue) {
    if state.active.is_none() {
        if !state.pending.is_empty() {
            let request = state.pending.remove(0);
            state.active = Some(open_modal(request, modals));
        }
    }
}

fn policy_action(
    scope: &DatasetScope,
    entries: &[&DatasetEntry],
    interactive: bool,
    show_on_start: bool,
) -> Option<String> {
    use lunco_hooks::HookValue as H;

    let datasets = entries
        .iter()
        .map(|entry| {
            H::map([
                ("id", H::str(entry.id.clone())),
                ("key", H::str(entry.key.clone())),
                ("name", H::str(entry.name.clone())),
                ("processed", H::Bool(entry.spec.process.is_some())),
            ])
        })
        .collect();
    let scope_kind = match scope {
        DatasetScope::Engine => "engine",
        DatasetScope::Twin { .. } => "twin",
    };
    let context = H::map([
        ("scope", H::str(scope_kind)),
        ("owner", H::str(scope.label())),
        ("interactive", H::Bool(interactive)),
        ("show_on_start", H::Bool(show_on_start)),
        ("datasets", H::Array(datasets)),
    ]);
    match lunco_hooks::invoke(lunco_core::session::DATASET_PROVISION_HOOK, &[context]) {
        Some(Ok(value)) => value.as_str().map(str::to_owned),
        Some(Err(error)) => {
            warn!("[datasets] provisioning policy failed: {error}");
            None
        }
        None => None,
    }
}

/// Event emitted by the popup or Settings menu when the project-level prompt
/// preference changes. The observer owns manifest mutation and persistence;
/// presentation code only emits this intent.
#[derive(Event, Clone, Debug)]
pub(crate) struct SetMissingAssetPromptSuppressed {
    pub(crate) root: PathBuf,
    pub(crate) suppressed: bool,
}

fn project_root_for_scope(
    scope: &DatasetScope,
    workspace: Option<&WorkspaceResource>,
) -> Option<PathBuf> {
    let root = match scope {
        DatasetScope::Twin { root, .. } => Some(root.clone()),
        DatasetScope::Engine => workspace.and_then(|workspace| {
            let id = workspace.active_twin?;
            workspace.twin(id).map(|twin| twin.root.clone())
        }),
    }?;
    workspace
        .filter(|workspace| {
            workspace
                .twins()
                .any(|(_, twin)| twin.root == root && twin.manifest.is_some())
        })
        .map(|_| root)
}

fn project_suppresses_prompt(root: Option<&Path>, workspace: Option<&WorkspaceResource>) -> bool {
    root.and_then(|root| {
        workspace.and_then(|workspace| {
            workspace
                .twins()
                .find(|(_, twin)| twin.root == root)
                .and_then(|(_, twin)| twin.manifest.as_ref())
        })
    })
    .is_some_and(lunco_twin::TwinManifest::suppress_missing_asset_prompt)
}

fn should_show_prompt(suppressed: bool) -> bool {
    !suppressed
}

/// Read the active project's missing-asset prompt preference for the Settings
/// menu. The returned root is the persistence key for the typed intent.
pub(crate) fn active_project_prompt_setting(
    workspace: Option<&WorkspaceResource>,
) -> Option<(PathBuf, bool)> {
    let workspace = workspace?;
    let id = workspace.active_twin?;
    let twin = workspace.twin(id)?;
    let manifest = twin.manifest.as_ref()?;
    Some((twin.root.clone(), manifest.suppress_missing_asset_prompt()))
}

/// Apply and persist a project-owned prompt preference through the loaded Twin
/// manifest. A plain folder without `twin.toml` has no project settings file,
/// so the intent is reported and ignored rather than silently creating one.
pub(crate) fn on_set_missing_asset_prompt_suppressed(
    trigger: On<SetMissingAssetPromptSuppressed>,
    mut workspace: ResMut<WorkspaceResource>,
) {
    let event = trigger.event();
    let Some((id, _)) = workspace.twins().find(|(_, twin)| twin.root == event.root) else {
        warn!(
            "[datasets] cannot save missing-asset prompt setting: project `{}` is not open",
            event.root.display()
        );
        return;
    };
    let Some(twin) = workspace.twin_mut(id) else {
        return;
    };
    let Some(manifest) = twin.manifest.as_mut() else {
        warn!(
            "[datasets] cannot save missing-asset prompt setting: `{}` has no twin.toml",
            event.root.display()
        );
        return;
    };
    if manifest.suppress_missing_asset_prompt() == event.suppressed {
        return;
    }
    manifest.downloads = event.suppressed.then(|| lunco_twin::DownloadManifest {
        suppress_missing_prompt: true,
    });
    if let Err(error) = twin.save_manifest() {
        warn!(
            "[datasets] could not save missing-asset prompt setting to `{}`: {error}",
            event.root.display()
        );
    }
}

pub(crate) fn on_dataset_scope_ready(
    trigger: On<DatasetScopeReady>,
    registry: Res<lunco_assets::datasets::DatasetRegistry>,
    workspace: Option<Res<WorkspaceResource>>,
    windows: Query<(), With<Window>>,
    mut state: ResMut<DatasetProvisioningState>,
    mut modals: ResMut<ModalQueue>,
) {
    let scope = &trigger.event().scope;
    if same_scope(&state, scope) {
        return;
    }
    let visible = visible_entries(&registry, scope);
    let missing: Vec<&DatasetEntry> = visible
        .iter()
        .copied()
        .filter(|entry| needs_provisioning(&entry.state))
        .collect();
    if missing.is_empty() {
        return;
    }

    let project_root = project_root_for_scope(scope, workspace.as_deref());
    let suppress_prompt = project_suppresses_prompt(project_root.as_deref(), workspace.as_deref());
    let show_on_start = should_show_prompt(suppress_prompt);
    if !show_on_start {
        return;
    }

    let choices: Vec<ProvisionedDataset> = visible
        .iter()
        .map(|entry| ProvisionedDataset {
            id: entry.id.clone(),
            name: entry.name.clone(),
            state: entry.state.clone(),
            has_processing: entry.spec.process.is_some(),
        })
        .collect();
    if policy_action(scope, &missing, !windows.is_empty(), show_on_start).as_deref()
        != Some("prompt")
    {
        return;
    }
    let selection = Arc::new(std::sync::Mutex::new(
        choices
            .iter()
            .map(|dataset| needs_provisioning(&dataset.state))
            .collect(),
    ));
    state.pending.push(ProvisioningRequest {
        scope: scope.clone(),
        datasets: choices,
        selection,
        project_root,
        suppress_prompt: Arc::new(std::sync::Mutex::new(false)),
    });
    show_next(&mut state, &mut modals);
}

pub(crate) fn on_dataset_scope_removed(
    trigger: On<DatasetScopeRemoved>,
    mut state: ResMut<DatasetProvisioningState>,
    mut modals: ResMut<ModalQueue>,
) {
    let scope = &trigger.event().scope;
    state.pending.retain(|request| &request.scope != scope);
    if state
        .active
        .as_ref()
        .is_some_and(|active| &active.request.scope == scope)
    {
        if let Some(active) = state.active.take() {
            modals.cancel(active.modal);
        }
    }
    show_next(&mut state, &mut modals);
}

pub(crate) fn poll_dataset_provisioning(
    mut state: ResMut<DatasetProvisioningState>,
    mut modals: ResMut<ModalQueue>,
    mut commands: Commands,
) {
    let Some(active) = state.active.as_ref() else {
        show_next(&mut state, &mut modals);
        return;
    };
    let Some(outcome) = modals.poll(active.modal) else {
        return;
    };
    let active = state
        .active
        .take()
        .expect("active provisioning modal exists");

    if matches!(outcome, ModalOutcome::Confirmed(_)) {
        if let Ok(selected) = active.request.selection.lock() {
            for (dataset, selected) in active.request.datasets.into_iter().zip(selected.iter()) {
                if *selected {
                    commands.trigger(RequestDataset { id: dataset.id });
                }
            }
        } else {
            warn!("[datasets] provisioning selection was unavailable; no downloads started");
        }
    }
    if let Some(root) = active.request.project_root {
        if let Ok(suppressed) = active.request.suppress_prompt.lock() {
            commands.trigger(SetMissingAssetPromptSuppressed {
                root,
                suppressed: *suppressed,
            });
        }
    }
    show_next(&mut state, &mut modals);
}

#[cfg(test)]
mod tests {
    use super::should_show_prompt;

    #[test]
    fn unchecked_negative_setting_keeps_prompt_enabled() {
        assert!(should_show_prompt(false));
        assert!(!should_show_prompt(true));
    }
}
