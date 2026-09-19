//! Render-free scene selection state.
//!
//! Selection is a scene-lifetime fact, not an editor widget.  The runtime,
//! exposure registry, and UI adapters all read the same resource, while this
//! package keeps that contract independent from the much larger scene mutation
//! command layer.

use bevy::prelude::*;

/// Read-only public view of the generic live-scene selection.
pub struct InspectSelectionProvider;

impl lunco_api::queries::ApiQueryProvider for InspectSelectionProvider {
    fn name(&self) -> &'static str {
        "InspectSelection"
    }

    fn execute(
        &self,
        world: &World,
        _params: &lunco_api_core::ApiValue,
    ) -> lunco_api::ApiQueryResult {
        let Some(selected) = world.get_resource::<SelectedEntities>() else {
            return Err(lunco_api::ApiQueryError::new(
                lunco_api_core::ApiErrorCode::InternalError,
                "InspectSelection: SelectedEntities resource is not present",
            ));
        };
        let Some(registry) = world.get_resource::<lunco_api::registry::ApiEntityRegistry>() else {
            return Err(lunco_api::ApiQueryError::new(
                lunco_api_core::ApiErrorCode::InternalError,
                "InspectSelection: ApiEntityRegistry resource is not present",
            ));
        };

        let selected_ids: Vec<u64> = selected
            .entities
            .iter()
            .filter_map(|entity| registry.api_id_for(*entity).map(|id| id.get()))
            .collect();
        let primary = selected_ids.last().copied();
        let stale_count = selected.entities.len() - selected_ids.len();
        Ok(Some(lunco_api_core::api_value!({
            "selected": selected_ids,
            "primary": primary,
            "paths": selected.stable_paths.clone(),
            "stale_count": stale_count,
        })))
    }
}

/// Entities selected in the active scene.
///
/// The last entity is the primary selection.  Entity ids are scene-scoped and
/// are cleared at the shared scene teardown boundary before a replacement can
/// reuse them.
#[derive(Resource, Default, Clone)]
pub struct SelectedEntities {
    /// Selected entities in insertion order; the last one is primary.
    pub entities: Vec<Entity>,
    /// Stable USD paths requested by generic scene-selection commands.
    ///
    /// Entities are disposable projections. Keeping the selected path beside
    /// the live entity lets a command-owned selection survive a structural
    /// re-projection without putting selection state in USD or in a
    /// feature-specific registry.
    pub stable_paths: Vec<String>,
}

impl SelectedEntities {
    /// Returns the primary selected entity, if any.
    pub fn primary(&self) -> Option<Entity> {
        self.entities.last().copied()
    }
}

/// Entity-keyed sub-selection used by editor panels and gizmos.
#[derive(Resource, Default)]
pub struct SelectionTarget {
    /// The targeted sub-part, or `None` for the complete selected object.
    pub part: Option<Entity>,
}

/// Semantic selection operation shared by viewport and panel input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionIntent {
    /// Replace the current selection with the target.
    Replace,
    /// Add the target while retaining the current selection.
    Extend,
    /// Toggle the target in the current selection.
    Toggle,
    /// Remove the target from the current selection.
    Remove,
}

/// Entity-keyed selection intent emitted by editor panels.
#[derive(Event, Clone, Copy)]
pub struct SelectEntityTarget {
    /// Entity selected by the editor gesture.
    pub target: Entity,
    /// Semantic selection operation to apply.
    pub intent: SelectionIntent,
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
            .init_resource::<lunco_api::queries::ApiQueryRegistry>()
            .init_resource::<lunco_signal::TelemetryFocus>()
            .add_systems(Update, mirror_selection_to_telemetry_focus)
            .add_systems(
                lunco_core::SceneTeardown,
                |mut selected: ResMut<SelectedEntities>| {
                    selected.entities.clear();
                    selected.stable_paths.clear();
                },
            );
        app.world_mut()
            .resource_mut::<lunco_api::queries::ApiQueryRegistry>()
            .register(InspectSelectionProvider);
    }
}
