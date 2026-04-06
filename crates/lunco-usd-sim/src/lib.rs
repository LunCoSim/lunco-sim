use bevy::prelude::*;
use bevy::math::DVec3;
use avian3d::prelude::*;
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
    if let Some(radius) = reader.get_prim_attribute_value::<f32>(&sdf_path, "physxVehicleWheel:radius") {
        info!("Intercepted PhysxVehicleWheelAPI for {}, injecting LunCo Raycast Wheel", prim_path.path);
        
        // INTERCEPT & SUBSTITUTE: 
        // We inject our specialized RaycastWheel which overrides standard 
        // rigid-body collision logic for high-performance mobility.
        let mut wheel = WheelRaycast {
            wheel_radius: radius as f64,
            visual_entity: Some(entity), // Point to itself for visual suspension offset
            ..default()
        };

        // 2. Map Suspension (from PhysxVehicleSuspensionAPI)
        if let Some(rest_len) = reader.get_prim_attribute_value::<f32>(&sdf_path, "physxVehicleSuspension:restLength") {
            wheel.rest_length = rest_len as f64;
        }
        if let Some(k) = reader.get_prim_attribute_value::<f32>(&sdf_path, "physxVehicleSuspension:springStiffness") {
            wheel.spring_k = k as f64;
        }
        if let Some(d) = reader.get_prim_attribute_value::<f32>(&sdf_path, "physxVehicleSuspension:springDamping") {
            wheel.damping_c = d as f64;
        }

        commands.entity(entity).insert((
            wheel,
            RayCaster::new(DVec3::ZERO, Dir3::NEG_Y),
            RayHits::default(),
        ));

        // 3. PRIORITY: Remove standard physics if they were added by other plugins
        // We want the wheel to be a raycast-only entity.
        commands.entity(entity)
            .remove::<Collider>()
            .remove::<RigidBody>()
            .remove::<Mass>();
    }
}
