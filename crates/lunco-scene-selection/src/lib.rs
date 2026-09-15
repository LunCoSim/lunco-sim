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
        _params: &serde_json::Value,
    ) -> lunco_api::schema::ApiResponse {
        let Some(selected) = world.get_resource::<SelectedEntities>() else {
            return lunco_api::schema::ApiResponse::error(
                lunco_api::schema::ApiErrorCode::InternalError,
                "InspectSelection: SelectedEntities resource is not present",
            );
        };
        let Some(registry) = world.get_resource::<lunco_api::registry::ApiEntityRegistry>() else {
            return lunco_api::schema::ApiResponse::error(
                lunco_api::schema::ApiErrorCode::InternalError,
                "InspectSelection: ApiEntityRegistry resource is not present",
            );
        };

        let selected_ids: Vec<u64> = selected
            .entities
            .iter()
            .filter_map(|entity| registry.api_id_for(*entity).map(|id| id.get()))
            .collect();
        lunco_api::schema::ApiResponse::ok(serde_json::json!({
            "selected": selected_ids,
            "primary": selected_ids.last().copied(),
            "paths": selected.stable_paths,
            "stale_count": selected.entities.len() - selected_ids.len(),
        }))
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
