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

    let reader = (*stage.reader).clone();

    // Map RigidBody
    if let Some(true) = reader.prim_attribute_value::<bool>(&sdf_path, "physics:rigidBodyEnabled") {
        commands.entity(entity).insert(RigidBody::Dynamic);
    }

    // Map Mass
    if let Some(mass) = reader.prim_attribute_value::<f32>(&sdf_path, "physics:mass") {
        commands.entity(entity).insert(Mass(mass));
    } else if let Some(mass) = reader.prim_attribute_value::<f64>(&sdf_path, "physics:mass") {
        commands.entity(entity).insert(Mass(mass as f32));
    }

    // Map Collider
    let collision_enabled = reader.prim_attribute_value::<bool>(&sdf_path, "physics:collisionEnabled").unwrap_or(true);

    if collision_enabled {
        if let Ok(val) = reader.get(&sdf_path, "typeName") {
            if let Value::Token(ty) = &*val {
                match ty.as_str() {
                    "Cube" => {
                        let width = reader.prim_attribute_value::<f64>(&sdf_path, "width")
                            .expect("Cube must have 'width' attribute");
                        let height = reader.prim_attribute_value::<f64>(&sdf_path, "height")
                            .expect("Cube must have 'height' attribute");
                        let depth = reader.prim_attribute_value::<f64>(&sdf_path, "depth")
                            .expect("Cube must have 'depth' attribute");
                        // Collider::cuboid expects half-extents
                        commands.entity(entity).insert(Collider::cuboid(width * 0.5, height * 0.5, depth * 0.5));
                    }
                    "Sphere" => {
                        let radius = reader.prim_attribute_value::<f64>(&sdf_path, "radius")
                            .expect("Sphere must have 'radius' attribute");
                        commands.entity(entity).insert(Collider::sphere(radius));
                    }
                    "Cylinder" => {
                        let radius = reader.prim_attribute_value::<f64>(&sdf_path, "radius")
                            .expect("Cylinder must have 'radius' attribute");
                        let height = reader.prim_attribute_value::<f64>(&sdf_path, "height")
                            .expect("Cylinder must have 'height' attribute");
                        commands.entity(entity).insert(Collider::cylinder(radius, height));
                    }
                    _ => {}
                }
            }
        }
    }
}
