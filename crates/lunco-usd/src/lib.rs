use bevy::prelude::*;
pub use lunco_usd_bevy::{UsdBevyPlugin, UsdPrimPath, UsdStageAsset};
pub use lunco_usd_avian::UsdAvianPlugin;
pub use lunco_usd_sim::UsdSimPlugin;

use openusd::sdf::{Path as SdfPath};
use lunco_core::Spacecraft;

/// A bundle plugin that adds all modular USD integration layers.
pub struct UsdPlugins;

impl Plugin for UsdPlugins {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            UsdBevyPlugin,
            UsdAvianPlugin,
            UsdPhysxPlugin,
            UsdLunCoPlugin,
        ));
    }
}

/// Plugin for mapping LunCo-specific engineering metadata from USD.
pub struct UsdLunCoPlugin;

impl Plugin for UsdLunCoPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_add_lunco_prim);
    }
}

fn on_add_lunco_prim(
    trigger: On<Add, UsdPrimPath>,
    query: Query<&UsdPrimPath>,
    stages: Res<Assets<UsdStageAsset>>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    let Ok(prim_path) = query.get(entity) else { return; };
    let Some(stage) = stages.get(&prim_path.stage_handle) else { return; };
    let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { return; };

    let mut reader = (*stage.reader).clone();
    let mut spacecraft = Spacecraft::default();
    let mut modified = false;

    // Use new type-safe getters from fork
    if let Some(name) = reader.get_prim_attribute_value::<String>(&sdf_path, "lunco:name") {
        spacecraft.name = name;
        modified = true;
    }

    if let Some(id) = reader.get_prim_attribute_value::<i32>(&sdf_path, "lunco:ephemeris_id") {
        spacecraft.ephemeris_id = id;
        modified = true;
    }

    if let Some(id) = reader.get_prim_attribute_value::<i32>(&sdf_path, "lunco:reference_id") {
        spacecraft.reference_id = id;
        modified = true;
    }

    if let Some(radius) = reader.get_prim_attribute_value::<f32>(&sdf_path, "lunco:hit_radius_m") {
        spacecraft.hit_radius_m = radius;
        modified = true;
    }

    if modified {
        commands.entity(entity).insert(spacecraft);
        info!("Mapped LunCo metadata for {}", prim_path.path);
    }
}
}
