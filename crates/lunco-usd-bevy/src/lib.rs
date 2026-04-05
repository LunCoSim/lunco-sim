use bevy::prelude::*;
use openusd::usda::TextReader;
use openusd::sdf::{AbstractData, Path as SdfPath, Value};

pub struct UsdBevyPlugin;

impl Plugin for UsdBevyPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<UsdPrimPath>()
            .add_systems(Update, sync_usd_visuals);
    }
}

#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
pub struct UsdPrimPath {
    pub stage_id: Entity,
    pub path: String,
}

impl Default for UsdPrimPath {
    fn default() -> Self {
        Self {
            stage_id: Entity::PLACEHOLDER,
            path: "/".to_string(),
        }
    }
}

#[derive(Component)]
pub struct UsdStageResource {
    pub reader: TextReader,
}

#[derive(Component)]
pub struct UsdVisualSynced;

fn sync_usd_visuals(
    mut commands: Commands,
    query: Query<(Entity, &UsdPrimPath), Without<UsdVisualSynced>>,
    mut stage_query: Query<&mut UsdStageResource>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, prim_path) in query.iter() {
        let Ok(mut stage_res) = stage_query.get_mut(prim_path.stage_id) else { continue; };
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { continue; };

        // 1. Detect Visual Mesh based on USD TypeName
        let mut mesh_handle = None;
        if let Ok(val) = stage_res.reader.get(&sdf_path, "typeName") {
            if let Value::Token(ty) = &*val {
                match ty.as_str() {
                    "Cube" => mesh_handle = Some(meshes.add(Cuboid::default())),
                    "Sphere" => mesh_handle = Some(meshes.add(Sphere::new(1.0).mesh().ico(32).unwrap())),
                    "Cylinder" => mesh_handle = Some(meshes.add(Cylinder::default())),
                    _ => {}
                }
            }
        }

        // 2. Map Color
        let mut color = Color::WHITE;
        if let Ok(prop_path) = sdf_path.append_property("primvars:displayColor") {
            if let Ok(val) = stage_res.reader.get(&prop_path, "default") {
                match &*val {
                    Value::Vec3f(v) if v.len() >= 3 => { color = Color::srgb(v[0], v[1], v[2]); }
                    _ => {}
                }
            }
        }

        if let Some(m) = mesh_handle {
            commands.entity(entity).insert((
                Mesh3d(m), 
                MeshMaterial3d(materials.add(StandardMaterial::from(color)))
            ));
        }

        // 3. Map Transform
        let mut transform = Transform::default();
        let mut get_attr = |name: &str| -> Option<Value> {
            if let Ok(prop_path) = sdf_path.append_property(name) {
                if let Ok(val) = stage_res.reader.get(&prop_path, "default") {
                    return Some((*val).clone());
                }
            }
            None
        };

        if let Some(val) = get_attr("xformOp:scale").or_else(|| get_attr("scale")) {
            match val {
                Value::Vec3f(v) if v.len() >= 3 => { transform.scale = Vec3::new(v[0], v[1], v[2]); }
                Value::Vec3d(v) if v.len() >= 3 => { transform.scale = Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32); }
                _ => {}
            }
        }
        if let Some(val) = get_attr("xformOp:translate").or_else(|| get_attr("translate")) {
            match val {
                Value::Vec3f(v) if v.len() >= 3 => { transform.translation = Vec3::new(v[0], v[1], v[2]); }
                Value::Vec3d(v) if v.len() >= 3 => { transform.translation = Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32); }
                _ => {}
            }
        }

        commands.entity(entity).insert((transform, UsdVisualSynced));
        info!("Successfully LOADED visuals from USD for {}", prim_path.path);

        // 4. CLEAN RECURSION using new fork API
        for child_path in stage_res.reader.get_children(&sdf_path) {
            let child_entity = commands.spawn((
                Name::new(child_path.to_string()),
                UsdPrimPath {
                    stage_id: prim_path.stage_id,
                    path: child_path.to_string(),
                },
                Transform::default(),
                Visibility::Inherited,
            )).id();
            commands.entity(entity).add_child(child_entity);
        }
    }
}
