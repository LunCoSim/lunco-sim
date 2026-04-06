use bevy::prelude::*;
use avian3d::prelude::*;
pub use lunco_usd_bevy::{UsdPrimPath, UsdStageAsset};
use openusd::sdf::{AbstractData, Path as SdfPath, Value};

pub struct UsdAvianPlugin;

impl Plugin for UsdAvianPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_add_usd_prim);
    }
}

fn on_add_usd_prim(
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

    // 1. Map RigidBody
    if let Some(true) = reader.get_prim_attribute_value::<bool>(&sdf_path, "physics:rigidBodyEnabled") {
        commands.entity(entity).insert(RigidBody::Dynamic);
        info!("Mapped {} to RigidBody::Dynamic", prim_path.path);
    }

    // 2. Map Mass
    if let Some(mass) = reader.get_prim_attribute_value::<f32>(&sdf_path, "physics:mass") {
        commands.entity(entity).insert(Mass(mass));
    } else if let Some(mass) = reader.get_prim_attribute_value::<f64>(&sdf_path, "physics:mass") {
        commands.entity(entity).insert(Mass(mass as f32));
    }

    // 3. Map Collider (Basic Primitives)
    // Check if collision is explicitly enabled or if it's a primitive mesh
    let collision_enabled = reader.get_prim_attribute_value::<bool>(&sdf_path, "physics:collisionEnabled").unwrap_or(true);
    
    if collision_enabled {
        if let Ok(val) = reader.get(&sdf_path, "typeName") {
            if let Value::Token(ty) = &*val {
                match ty.as_str() {
                    "Cube" => {
                        // USD default size is 1.0, but we should respect scale
                        commands.entity(entity).insert(Collider::cuboid(1.0, 1.0, 1.0));
                    }
                    "Sphere" => {
                        // USD default radius is 0.5
                        let radius = reader.get_prim_attribute_value::<f64>(&sdf_path, "radius").unwrap_or(0.5);
                        commands.entity(entity).insert(Collider::sphere(radius));
                    }
                    "Cylinder" => {
                        // USD default radius is 0.5, height is 1.0
                        let radius = reader.get_prim_attribute_value::<f64>(&sdf_path, "radius").unwrap_or(0.5);
                        let height = reader.get_prim_attribute_value::<f64>(&sdf_path, "height").unwrap_or(1.0);
                        commands.entity(entity).insert(Collider::cylinder(radius, height));
                    }
                    _ => {}
                }
            }
        }
    }
}
