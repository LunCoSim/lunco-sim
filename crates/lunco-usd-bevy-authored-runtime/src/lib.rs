//! Reusable adapter for generic behavior authored in USD.
//!
//! This package consumes the generic scene projection boundary and attaches
//! authored control surfaces and executable `LunCoProgramAPI` children. It
//! owns no input device policy, scene admission, or visual projection.

use bevy::prelude::{App, IntoScheduleConfigs, Plugin, Update};

mod control_runtime;
mod program_runtime;

/// Install authored control and generic-program projection after visual USD
/// projection has admitted a scene entity.
pub struct UsdAuthoredRuntimePlugin;

impl Plugin for UsdAuthoredRuntimePlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<lunco_camera_core::CameraFollow>();
        app.add_observer(control_runtime::queue_authored_runtime_projection);
        app.add_systems(
            Update,
            control_runtime::project_authored_runtime_components
                .after(lunco_usd_bevy_scene::UsdVisualProjectionSet)
                .run_if(control_runtime::has_pending_authored_runtime_projection),
        );
    }
}

/// Refresh one owner's generic authored program attachment after a live USD
/// structural or source change.
pub fn refresh_program_owner(
    world: &mut bevy::prelude::World,
    stage_id: bevy::asset::AssetId<lunco_usd_bevy_stage::UsdStageAsset>,
    owner: bevy::prelude::Entity,
) {
    program_runtime::refresh_program_owner(world, stage_id, owner);
}
