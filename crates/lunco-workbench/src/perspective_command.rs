//! Typed task activation and explicit layout reset commands.
//! Selecting the active task preserves its dock. ResetWorkspaceLayout reapplies
//! the current authored preset, while task navigation restores visited layouts.

use bevy::prelude::*;
use lunco_core::{on_command, register_commands};
use lunco_workbench_core::commands::{
    ActivatePerspective, ResetToDefaultPerspective, ResetWorkspaceLayout, SetRequiredPerspective,
};
use lunco_workbench_layout::WorkbenchLayout;

#[on_command(ActivatePerspective)]
fn on_activate_perspective(
    trigger: On<ActivatePerspective>,
    layout: Option<ResMut<WorkbenchLayout>>,
    mut snapshot: Option<ResMut<crate::WorkbenchSnapshot>>,
    pending: Option<ResMut<crate::PendingLayoutRequests>>,
    mut commands: Commands,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut choice: Option<ResMut<lunco_workbench_core::ExplicitPerspectiveChoice>>,
) {
    let id = trigger.event().id.clone();
    let Some(mut layout) = layout else {
        if let Some(mut pending) = pending {
            pending
                .0
                .push(crate::LayoutRequest::ActivatePerspective(id));
        }
        return;
    };
    if layout.activate_perspective_by_str(&id) {
        crate::record_explicit_perspective(&layout, workspace.as_deref(), choice.as_deref_mut());
        if let Some(snapshot) = snapshot.as_deref_mut() {
            if let Some(next) = crate::snapshot_for_layout_if_changed(&layout, snapshot) {
                *snapshot = next;
            }
        }
        info!("[ActivatePerspective] activated `{id}`");
    } else {
        warn!("[ActivatePerspective] no registered perspective with id `{id}`");
        report_unknown_perspective(&mut commands, &id);
    }
}

pub(crate) fn report_unknown_perspective(commands: &mut Commands, id: &str) {
    commands.trigger(lunco_telemetry_core::TelemetryEvent {
        name: "perspective-activation-failed".to_string(),
        source: 0,
        severity: lunco_telemetry_core::Severity::Error,
        data: lunco_telemetry_core::TelemetryValue::String(format!(
            "No workbench perspective named `{id}` is registered"
        )),
        timestamp: 0.0,
        sim_secs: 0.0,
        sim_tick: 0,
    });
}

#[on_command(ResetWorkspaceLayout)]
fn on_reset_workspace_layout(
    _trigger: On<ResetWorkspaceLayout>,
    layout: Option<ResMut<WorkbenchLayout>>,
    mut snapshot: Option<ResMut<crate::WorkbenchSnapshot>>,
    pending: Option<ResMut<crate::PendingLayoutRequests>>,
) {
    let Some(mut layout) = layout else {
        if let Some(mut pending) = pending {
            pending.0.push(crate::LayoutRequest::Reset);
        }
        return;
    };
    layout.reset_to_default_layout();
    if let Some(snapshot) = snapshot.as_deref_mut() {
        if let Some(next) = crate::snapshot_for_layout_if_changed(&layout, snapshot) {
            *snapshot = next;
        }
    }
    info!("[ResetWorkspaceLayout] dock reset to active perspective preset");
}

#[on_command(SetRequiredPerspective)]
fn on_set_required_perspective(
    trigger: On<SetRequiredPerspective>,
    layout: Option<ResMut<WorkbenchLayout>>,
) {
    let Some(mut layout) = layout else {
        return;
    };
    layout.set_required_perspective(trigger.event().id.as_deref());
}

#[on_command(ResetToDefaultPerspective)]
fn on_reset_to_default_perspective(
    _trigger: On<ResetToDefaultPerspective>,
    layout: Option<ResMut<WorkbenchLayout>>,
    mut snapshot: Option<ResMut<crate::WorkbenchSnapshot>>,
) {
    let Some(mut layout) = layout else {
        return;
    };
    layout.reset_to_default_perspective();
    if let Some(snapshot) = snapshot.as_deref_mut() {
        if let Some(next) = crate::snapshot_for_layout_if_changed(&layout, snapshot) {
            *snapshot = next;
        }
    }
}

register_commands!(
    on_activate_perspective,
    on_reset_workspace_layout,
    on_set_required_perspective,
    on_reset_to_default_perspective,
);

/// Plugin registering the [`ActivatePerspective`] command observer.
pub struct PerspectiveCommandPlugin;

impl Plugin for PerspectiveCommandPlugin {
    fn build(&self, app: &mut App) {
        register_all_commands(app);
    }
}
