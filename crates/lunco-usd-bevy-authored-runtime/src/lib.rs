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
        app.add_systems(
            Update,
            control_runtime::project_authored_runtime_components
                .after(lunco_usd_bevy_scene::UsdVisualProjectionSet),
        );
    }
}

/// Refresh one owner's generic authored program attachment after a live USD
/// structural or source change.
pub fn refresh_program_owner(
    world: &mut bevy::prelude::World,
    stage_id: bevy::asset::AssetId<lunco_usd_bevy_core::UsdStageAsset>,
    owner: bevy::prelude::Entity,
) {
    program_runtime::refresh_program_owner(world, stage_id, owner);
}
