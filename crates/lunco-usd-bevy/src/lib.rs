use bevy::prelude::*;
use bevy::asset::{AssetLoader, LoadContext, io::Reader};
use openusd::usda::TextReader;
use openusd::sdf::{AbstractData, Path as SdfPath, Value};
use lunco_usd_composer::UsdComposer;
use std::sync::Arc;

pub struct UsdBevyPlugin;

impl Plugin for UsdBevyPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<UsdStageAsset>()
            .register_asset_loader(UsdLoader)
            .register_type::<UsdPrimPath>()
            .add_systems(Update, sync_usd_visuals);
    }
}

/// A Bevy Asset representing a loaded USD Stage.
#[derive(Asset, TypePath, Clone)]
pub struct UsdStageAsset {
    pub reader: Arc<TextReader>,
}

#[derive(Default, TypePath)]
pub struct UsdLoader;

impl AssetLoader for UsdLoader {
    type Asset = UsdStageAsset;
    type Settings = ();
    type Error = anyhow::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let data = String::from_utf8(bytes)?;

        let mut parser = openusd::usda::parser::Parser::new(&data);
        let data_map = parser.parse().map_err(|e| anyhow::anyhow!("USD Parse Error: {}", e))?;
        let reader = TextReader::from_data(data_map);

        // Resolve external references
        let reader = if let Some(parent) = load_context.path().path().parent() {
            let asset_root = std::path::Path::new("assets");
            // Use asset root as base_dir for reference resolution
            let base_dir = if asset_root.exists() { 
                asset_root.to_path_buf() 
            } else { 
                parent.to_path_buf() 
            };
            UsdComposer::flatten(&reader, &base_dir).map_err(|e| anyhow::anyhow!("USD Composition Error: {}", e))?
        } else {
            reader
        };

        Ok(UsdStageAsset {
            reader: Arc::new(reader),
        })
    }

    fn extensions(&self) -> &[&str] {
        &["usda"]
    }
}

#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
pub struct UsdPrimPath {
    pub stage_handle: Handle<UsdStageAsset>,
    pub path: String,
}

impl Default for UsdPrimPath {
    fn default() -> Self {
        Self {
            stage_handle: Handle::default(),
            path: "/".to_string(),
        }
    }
}

#[derive(Component)]
pub struct UsdVisualSynced;

pub fn sync_usd_visuals(
    mut commands: Commands,
    query: Query<(Entity, &UsdPrimPath, Option<&Visibility>, Option<&Transform>), Without<UsdVisualSynced>>,
    stages: Res<Assets<UsdStageAsset>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, prim_path, existing_vis, existing_tf) in query.iter() {
        let Some(stage) = stages.get(&prim_path.stage_handle) else { continue; };
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { continue; };

        let reader = (*stage.reader).clone();

        // Skip inactive prims
        if let Ok(val) = reader.get(&sdf_path, "active") {
            if let Value::Bool(active) = &*val {
                if !*active {
                    commands.entity(entity).insert(UsdVisualSynced);
                    return;
                }
            }
        }

        // Get prim type
        let prim_type = if let Ok(val) = reader.get(&sdf_path, "typeName") {
            if let Value::Token(ty) = &*val {
                Some(ty.clone())
            } else {
                None
            }
        } else {
            None
        };

        // Create mesh based on prim type and explicit dimensions
        let mesh_handle = match prim_type.as_deref() {
            Some("Cube") => {
                if let (Some(width), Some(height), Some(depth)) = (
                    reader.prim_attribute_value::<f64>(&sdf_path, "width"),
                    reader.prim_attribute_value::<f64>(&sdf_path, "height"),
                    reader.prim_attribute_value::<f64>(&sdf_path, "depth"),
                ) {
                    Some(meshes.add(Cuboid::new(width as f32 * 0.5, height as f32 * 0.5, depth as f32 * 0.5)))
                } else { None }
            }
            Some("Sphere") => {
                if let Some(radius) = reader.prim_attribute_value::<f64>(&sdf_path, "radius") {
                    Some(meshes.add(Sphere::new(radius as f32).mesh().ico(32).unwrap()))
                } else { None }
            }
            Some("Cylinder") => {
                if let (Some(radius), Some(height)) = (
                    reader.prim_attribute_value::<f64>(&sdf_path, "radius"),
                    reader.prim_attribute_value::<f64>(&sdf_path, "height"),
                ) {
                    Some(meshes.add(Cylinder::new(radius as f32, height as f32)))
                } else { None }
            }
            _ => None,
        };

        // Get color
        let color = get_attribute_as_vec3(&reader, &sdf_path, "primvars:displayColor")
            .map(|v| Color::srgb(v.x, v.y, v.z))
            .unwrap_or(Color::WHITE);

        if let Some(ref m) = mesh_handle {
            commands.entity(entity).insert((
                Mesh3d(m.clone()),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: color,
                    ..default()
                }))
            ));
        }

        // Transform (position and rotation)
        let mut transform = existing_tf.cloned().unwrap_or_default();
        if let Some(v) = get_attribute_as_vec3(&reader, &sdf_path, "xformOp:translate") {
            transform.translation = v;
        }
        if let Some(v) = get_attribute_as_vec3(&reader, &sdf_path, "xformOp:rotateXYZ") {
            // USD stores rotation in degrees; convert to radians for Bevy
            let rx = v.x.to_radians();
            let ry = v.y.to_radians();
            let rz = v.z.to_radians();
            transform.rotation = Quat::from_euler(EulerRot::XYZ, rx, ry, rz);
        }

        let final_vis = existing_vis.cloned().unwrap_or(Visibility::Inherited);

        commands.entity(entity).insert((
            transform,
            UsdVisualSynced,
            final_vis,
            InheritedVisibility::default(),
            ViewVisibility::default(),
        ));

        // Spawn children
        for child_path in reader.prim_children(&sdf_path) {
            if let Ok(val) = reader.get(&child_path, "active") {
                if let Value::Bool(active) = &*val {
                    if !*active { continue; }
                }
            }

            let child_entity = commands.spawn((
                Name::new(child_path.to_string()),
                UsdPrimPath {
                    stage_handle: prim_path.stage_handle.clone(),
                    path: child_path.to_string(),
                },
                Visibility::Inherited,
                InheritedVisibility::default(),
                ViewVisibility::default(),
            )).id();
            commands.entity(entity).add_child(child_entity);
        }
    }
}

fn get_attribute_as_vec3(reader: &TextReader, path: &SdfPath, attr: &str) -> Option<Vec3> {
    if let Some(v) = reader.prim_attribute_value::<Vec<f32>>(path, attr) {
        if v.len() >= 3 { return Some(Vec3::new(v[0], v[1], v[2])); }
    }
    if let Some(v) = reader.prim_attribute_value::<Vec<f64>>(path, attr) {
        if v.len() >= 3 { return Some(Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32)); }
    }
    None
}
