//! Interactive consent for declared datasets.
//!
//! The asset registry owns identity, storage, downloading, processing, and
//! cancellation. This module owns only the windowed presentation and invokes
//! the authored provisioning policy with registry facts. A button dispatches
//! the same typed dataset command used by every other UI and script.

use std::sync::Arc;

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_assets::datasets::{
    DatasetEntry, DatasetScope, DatasetScopeReady, DatasetScopeRemoved, DatasetState,
    RequestDataset,
};
use lunco_ui::modal::{ModalBody, ModalButton, ModalId, ModalOutcome, ModalQueue, ModalRequest};

#[derive(Resource, Default)]
pub(crate) struct DatasetProvisioningState {
    pending: Vec<ProvisioningRequest>,
    active: Option<ActiveProvisioning>,
}

struct ProvisioningRequest {
    scope: DatasetScope,
    datasets: Vec<ProvisionedDataset>,
    selection: Arc<std::sync::Mutex<Vec<bool>>>,
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
        .filter(|entry| match scope {
            DatasetScope::Engine => entry.recommended,
            DatasetScope::Twin { .. } => true,
        })
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
        ui.add_space(2.0);
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
        ui.add_space(4.0);
        ui.label("Already ready resources are shown for visibility and cannot be selected.");
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

pub(crate) fn on_dataset_scope_ready(
    trigger: On<DatasetScopeReady>,
    registry: Res<lunco_assets::datasets::DatasetRegistry>,
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

    let choices: Vec<ProvisionedDataset> = visible
        .iter()
        .map(|entry| ProvisionedDataset {
            id: entry.id.clone(),
            name: entry.name.clone(),
            state: entry.state.clone(),
            has_processing: entry.spec.process.is_some(),
        })
        .collect();
    if policy_action(scope, &missing, !windows.is_empty()).as_deref() != Some("prompt") {
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
    show_next(&mut state, &mut modals);
}
