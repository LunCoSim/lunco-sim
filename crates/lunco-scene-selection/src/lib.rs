//! Render-free scene selection state.
//!
//! Selection is a scene-lifetime fact, not an editor widget.  The runtime,
//! exposure registry, and UI adapters all read the same resource, while this
//! package keeps that contract independent from the much larger scene mutation
//! command layer.

use bevy::prelude::*;

/// Entities selected in the active scene.
///
/// The last entity is the primary selection.  Entity ids are scene-scoped and
/// are cleared at the shared scene teardown boundary before a replacement can
/// reuse them.
#[derive(Resource, Default, Clone)]
pub struct SelectedEntities {
    /// Selected entities in insertion order; the last one is primary.
    pub entities: Vec<Entity>,
}

impl SelectedEntities {
    /// Returns the primary selected entity, if any.
    pub fn primary(&self) -> Option<Entity> {
        self.entities.last().copied()
    }
}

/// Mirrors selection into the render-free telemetry focus consumed by runtime
/// exposure and telemetry surfaces.
fn mirror_selection_to_telemetry_focus(
    selected: Res<SelectedEntities>,
    mut focus: ResMut<lunco_signal::TelemetryFocus>,
) {
    if selected.is_changed() && focus.roots != selected.entities {
        focus.roots.clone_from(&selected.entities);
    }
}

/// Installs the shared selection resource and its scene-lifetime cleanup.
pub struct SceneSelectionPlugin;

impl Plugin for SceneSelectionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SelectedEntities>()
            .init_resource::<lunco_signal::TelemetryFocus>()
            .add_systems(Update, mirror_selection_to_telemetry_focus)
            .add_systems(
                lunco_core::SceneTeardown,
                |mut selected: ResMut<SelectedEntities>| {
                    selected.entities.clear();
                },
            );
    }
}
