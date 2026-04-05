use bevy::prelude::*;
pub use lunco_usd_bevy::{UsdBevyPlugin, UsdPrimPath, UsdStageResource};
pub use lunco_usd_avian::UsdAvianPlugin;
pub use lunco_usd_physx::UsdPhysxPlugin;

use openusd::sdf::{AbstractData, Path as SdfPath, Value};
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
    mut stage_query: Query<&mut UsdStageResource>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    let Ok(prim_path) = query.get(entity) else { return; };
    let Ok(mut stage_res) = stage_query.get_mut(prim_path.stage_id) else { return; };
    let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { return; };

    let mut spacecraft = Spacecraft::default();
    let mut modified = false;

    // Helper to get custom lunco attributes
    let mut get_lunco_attr = |name: &str| -> Option<Value> {
        if let Ok(prop_path) = sdf_path.append_property(&format!("lunco:{name}")) {
            if let Ok(val) = stage_res.reader.get(&prop_path, "default") {
                return Some((*val).clone());
            }
        }
        None
    };

    if let Some(Value::String(name)) = get_lunco_attr("name") {
        spacecraft.name = name;
        modified = true;
    }

    if let Some(Value::Int(id)) = get_lunco_attr("ephemeris_id") {
        spacecraft.ephemeris_id = id;
        modified = true;
    }

    if let Some(Value::Int(id)) = get_lunco_attr("reference_id") {
        spacecraft.reference_id = id;
        modified = true;
    }

    if let Some(Value::Float(radius)) = get_lunco_attr("hit_radius_m") {
        spacecraft.hit_radius_m = radius;
        modified = true;
    }

    if modified {
        commands.entity(entity).insert(spacecraft);
        info!("Mapped LunCo metadata for {}", prim_path.path);
    }
}
