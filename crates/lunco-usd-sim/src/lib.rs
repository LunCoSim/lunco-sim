use bevy::prelude::*;
pub use lunco_usd_bevy::{UsdPrimPath, UsdStageAsset};
use openusd::sdf::{Path as SdfPath};
use lunco_mobility::WheelRaycast;

/// Plugin for mapping simulation-specific USD schemas (like NVIDIA PhysX Vehicles)
/// to LunCo's optimized simulation models.
pub struct UsdSimPlugin;

impl Plugin for UsdSimPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_add_usd_sim_prim);
    }
}

fn on_add_usd_sim_prim(
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

    // 1. Detect PhysxVehicleWheelAPI (The "Duck Typing" Intercept)
    // In USD, schemas are identified by their namespace. We check for a 
    // core attribute from the WheelAPI to identify the mesh as a functional wheel.
    if let Some(radius) = reader.get_prim_attribute_value::<f32>(&sdf_path, "physxVehicleWheel:radius") {
        info!("Intercepted PhysxVehicleWheelAPI for {}, injecting LunCo Raycast Wheel", prim_path.path);
        
        // INTERCEPT & SUBSTITUTE: 
        // We inject our specialized RaycastWheel which overrides standard 
        // rigid-body collision logic for high-performance mobility.
        commands.entity(entity).insert(WheelRaycast {
            wheel_radius: radius as f64,
            ..default()
        });
    }

    // TODO: Add mappings for PhysxVehicleTireAPI and PhysxVehicleSuspensionAPI
}
