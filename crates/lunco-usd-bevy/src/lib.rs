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

        // RESOLVE REFERENCES
        let reader = if let Some(parent) = load_context.path().path().parent() {
            let asset_root = std::path::Path::new("assets");
            let full_parent = asset_root.join(parent);
            let base_dir = if asset_root.exists() { full_parent } else { parent.to_path_buf() };
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

fn sync_usd_visuals(
    mut commands: Commands,
    query: Query<(Entity, &UsdPrimPath, Option<&Visibility>, Option<&Transform>), Without<UsdVisualSynced>>,
    stages: Res<Assets<UsdStageAsset>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, prim_path, existing_vis, existing_tf) in query.iter() {
        let Some(stage) = stages.get(&prim_path.stage_handle) else { continue; };
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { continue; };

        let mut reader = (*stage.reader).clone();
        
        // 0. Check Active status
        if let Ok(val) = reader.get(&sdf_path, "active") {
            if let Value::Bool(active) = &*val {
                if !*active {
                    debug!("Skipping inactive prim and its subtree: {}", prim_path.path);
                    commands.entity(entity).insert(UsdVisualSynced); 
                    return; // EXIT EARLY - don't process visuals OR children
                }
            }
        }

        // 1. Detect Visual Mesh
        let mut mesh_handle = None;
        let base_rotation = Quat::IDENTITY;
        
        // Try getting typeName from metadata or attributes
        let mut prim_type = None;
        if let Ok(val) = reader.get(&sdf_path, "typeName") {
            if let Value::Token(ty) = &*val {
                prim_type = Some(ty.to_string());
            }
        }

        // HEURISTIC: If typeName is missing, try to infer from attributes
        if prim_type.is_none() {
            if reader.get(&sdf_path, "size").is_ok() {
                prim_type = Some("Cube".to_string());
            } else if reader.get(&sdf_path, "height").is_ok() {
                prim_type = Some("Cylinder".to_string());
            } else if reader.get(&sdf_path, "radius").is_ok() {
                prim_type = Some("Sphere".to_string());
            }
        }

        if let Some(ty) = prim_type {
            match ty.as_str() {
                "Cube" => {
                    let size = reader.prim_attribute_value::<f64>(&sdf_path, "size").unwrap_or(1.0) as f32;
                    mesh_handle = Some(meshes.add(Cuboid::from_size(Vec3::splat(size))));
                }
                "Sphere" => {
                    let radius = reader.prim_attribute_value::<f64>(&sdf_path, "radius").unwrap_or(0.5) as f32;
                    mesh_handle = Some(meshes.add(Sphere::new(radius).mesh().ico(32).unwrap()));
                }
                "Cylinder" => {
                    let radius = reader.prim_attribute_value::<f64>(&sdf_path, "radius").unwrap_or(0.5) as f32;
                    let height = reader.prim_attribute_value::<f64>(&sdf_path, "height").unwrap_or(1.0) as f32;
                    mesh_handle = Some(meshes.add(Cylinder::new(radius, height)));
                }
                _ => {}
            }
        }

        // 2. Map Color
        let mut color = Color::WHITE;
        if let Some(v) = get_attribute_as_vec3(&mut reader, &sdf_path, "primvars:displayColor") {
            color = Color::srgb(v.x, v.y, v.z);
        }

        if let Some(ref m) = mesh_handle {
            commands.entity(entity).insert((
                Mesh3d(m.clone()), 
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: color,
                    ..default()
                }))
            ));
        }

        // 3. Map Transform
        let mut transform = existing_tf.cloned().unwrap_or_default();
        let mut has_usd_tf = false;
        
        if let Some(v) = get_attribute_as_vec3(&mut reader, &sdf_path, "xformOp:scale")
            .or_else(|| get_attribute_as_vec3(&mut reader, &sdf_path, "scale")) 
        {
            transform.scale = v;
            has_usd_tf = true;
        }
        if let Some(v) = get_attribute_as_vec3(&mut reader, &sdf_path, "xformOp:translate")
            .or_else(|| get_attribute_as_vec3(&mut reader, &sdf_path, "translate"))
        {
            // If it's the root prim and we have an existing transform, we might want to offset
            // But for now, if USD has a translation, it's local.
            transform.translation = v;
            has_usd_tf = true;
        }
        if let Some(v) = get_attribute_as_vec3(&mut reader, &sdf_path, "xformOp:rotateXYZ")
            .or_else(|| get_attribute_as_vec3(&mut reader, &sdf_path, "rotate"))
        {
            transform.rotation = Quat::from_euler(EulerRot::XYZ, v.x.to_radians(), v.y.to_radians(), v.z.to_radians()) * base_rotation;
            has_usd_tf = true;
        } else if has_usd_tf {
            transform.rotation = base_rotation;
        }

        let final_vis = existing_vis.cloned().unwrap_or(Visibility::Inherited);

        commands.entity(entity).insert((
            transform, 
            UsdVisualSynced, 
            final_vis,
            InheritedVisibility::default(),
            ViewVisibility::default()
        ));

        // 4. Recursion
        for child_path in reader.prim_children(&sdf_path) {
            // Check if child is active before spawning
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

fn get_attribute_as_vec3(reader: &mut TextReader, path: &SdfPath, attr: &str) -> Option<Vec3> {
    if let Some(v) = reader.prim_attribute_value::<Vec<f32>>(path, attr) {
        if v.len() >= 3 { return Some(Vec3::new(v[0], v[1], v[2])); }
    }
    if let Some(v) = reader.prim_attribute_value::<Vec<f64>>(path, attr) {
        if v.len() >= 3 { return Some(Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32)); }
    }
    None
}
