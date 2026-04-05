use bevy::prelude::*;
use avian3d::prelude::*;
pub use lunco_usd_bevy::{UsdPrimPath, UsdStageResource};
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
    mut stage_query: Query<&mut UsdStageResource>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    let Ok(prim_path) = query.get(entity) else { return; };
    let Ok(mut stage_res) = stage_query.get_mut(prim_path.stage_id) else { return; };
    let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { return; };

    // 1. Map RigidBody
    if let Ok(prop_path) = sdf_path.append_property("physics:rigidBodyEnabled") {
        if let Ok(val) = stage_res.reader.get(&prop_path, "default") {
            if let Value::Bool(true) = *val {
                commands.entity(entity).insert(RigidBody::Dynamic);
                info!("Mapped {} to RigidBody::Dynamic", prim_path.path);
            }
        }
    }

    // 2. Map Mass
    if let Ok(prop_path) = sdf_path.append_property("physics:mass") {
        if let Ok(val) = stage_res.reader.get(&prop_path, "default") {
            let mass_val = match &*val {
                Value::Float(m) => Some(*m),
                Value::Double(m) => Some(*m as f32),
                _ => None,
            };
            if let Some(m) = mass_val {
                commands.entity(entity).insert(Mass(m));
            }
        }
    }

    // 3. Map Collider (Basic)
    if let Ok(val) = stage_res.reader.get(&sdf_path, "typeName") {
        if let Value::Token(ty) = &*val {
            match ty.as_str() {
                "Cube" => {
                    commands.entity(entity).insert(Collider::cuboid(1.0, 1.0, 1.0));
                }
                "Cylinder" => {
                    commands.entity(entity).insert(Collider::cylinder(0.5, 1.0));
                }
                _ => {}
            }
        }
    }
}
