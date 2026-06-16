//! `ActivatePerspective` API command — switch the active workbench
//! [`Perspective`](crate::Perspective) (a named layout preset) from the
//! HTTP/script bus, mirroring the View / Build / Design buttons in the title
//! bar.
//!
//! Why this exists as a command: the buttons call
//! [`WorkbenchLayout::activate_perspective`] directly inside the egui draw, so
//! there was no way to switch layouts headlessly (test loops, agents driving
//! the native window over `/api/commands`). Activating a perspective at runtime
//! also **rebuilds the dock from the preset**, which restores a panel (e.g. the
//! 3D `ViewportPanel`) that a stale persisted workspace-state had dropped — the
//! persisted restore only runs at load, so a runtime re-activation wins.

use bevy::prelude::*;
use lunco_core::{Command, on_command, register_commands};
use crate::WorkbenchLayout;

/// Activate a registered [`Perspective`](crate::Perspective) by its
/// `PerspectiveId` string. The sandbox ships `sandbox_view`, `rover_build`,
/// and `modelica_analyze`. Unknown ids are a logged no-op.
#[Command(default)]
pub struct ActivatePerspective {
    /// The id string of a registered perspective (e.g. `"rover_build"`).
    pub id: String,
}

#[on_command(ActivatePerspective)]
fn on_activate_perspective(
    trigger: On<ActivatePerspective>,
    mut layout: ResMut<WorkbenchLayout>,
) {
    let id = trigger.event().id.clone();
    if layout.activate_perspective_by_str(&id) {
        info!("[ActivatePerspective] activated `{id}`");
    } else {
        warn!("[ActivatePerspective] no registered perspective with id `{id}`");
    }
}

/// Reset the dock layout to the active perspective's clean preset — the
/// recovery hatch when a stale persisted layout drops a panel (e.g. the 3D
/// Viewport, which leaves the centre blank and the camera inactive). Exposed on
/// the API bus and as the **View ▸ Reset Layout** menu item.
#[Command(default)]
pub struct ResetWorkspaceLayout {}

#[on_command(ResetWorkspaceLayout)]
fn on_reset_workspace_layout(
    _trigger: On<ResetWorkspaceLayout>,
    mut layout: ResMut<WorkbenchLayout>,
) {
    layout.reset_to_default_layout();
    info!("[ResetWorkspaceLayout] dock reset to active perspective preset");
}

register_commands!(on_activate_perspective, on_reset_workspace_layout);

/// Plugin registering the [`ActivatePerspective`] command observer.
pub struct PerspectiveCommandPlugin;

impl Plugin for PerspectiveCommandPlugin {
    fn build(&self, app: &mut App) {
        register_all_commands(app);
    }
}
