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
        let mut reader = TextReader::from_data(data_map);

        // RESOLVE REFERENCES
        if let Some(parent) = load_context.path().path().parent() {
            let asset_root = std::path::Path::new("assets");
            let full_parent = asset_root.join(parent);
            let base_dir = if asset_root.exists() { full_parent } else { parent.to_path_buf() };
            UsdComposer::flatten(&mut reader, &base_dir).map_err(|e| anyhow::anyhow!("USD Composition Error: {}", e))?;
        }

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
    query: Query<(Entity, &UsdPrimPath, Option<&Visibility>), Without<UsdVisualSynced>>,
    stages: Res<Assets<UsdStageAsset>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, prim_path, existing_vis) in query.iter() {
        let Some(stage) = stages.get(&prim_path.stage_handle) else { continue; };
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { continue; };

        let mut reader = (*stage.reader).clone();

        // 1. Detect Visual Mesh
        let mut mesh_handle = None;
        let base_rotation = Quat::IDENTITY;
        if let Ok(val) = reader.get(&sdf_path, "typeName") {
            if let Value::Token(ty) = &*val {
                match ty.as_str() {
                    "Cube" => {
                        // Use Bevy default size 1.0
                        let size = reader.get_prim_attribute_value::<f64>(&sdf_path, "size").unwrap_or(1.0) as f32;
                        mesh_handle = Some(meshes.add(Cuboid::from_size(Vec3::splat(size))));
                    }
                    "Sphere" => {
                        // Use Bevy default radius 0.5
                        let radius = reader.get_prim_attribute_value::<f64>(&sdf_path, "radius").unwrap_or(0.5) as f32;
                        mesh_handle = Some(meshes.add(Sphere::new(radius).mesh().ico(32).unwrap()));
                    }
                    "Cylinder" => {
                        // Use Bevy default radius 0.5, height 1.0
                        let radius = reader.get_prim_attribute_value::<f64>(&sdf_path, "radius").unwrap_or(0.5) as f32;
                        let height = reader.get_prim_attribute_value::<f64>(&sdf_path, "height").unwrap_or(1.0) as f32;
                        mesh_handle = Some(meshes.add(Cylinder::new(radius, height)));
                        
                        // Note: USD default axis is Z, but we default to Y-up to match Bevy
                        // and common asset authoring expectations in this project.
                    }
                    _ => {}
                }
            }
        }

        // 2. Map Color
        let mut color = Color::WHITE;
        if let Some(v) = get_attribute_as_vec3(&mut reader, &sdf_path, "primvars:displayColor") {
            color = Color::srgb(v.x, v.y, v.z);
        }

        if let Some(m) = mesh_handle {
            commands.entity(entity).insert((
                Mesh3d(m), 
                MeshMaterial3d(materials.add(StandardMaterial::from(color)))
            ));
        }

        // 3. Map Transform
        let mut transform = Transform::default();
        if let Some(v) = get_attribute_as_vec3(&mut reader, &sdf_path, "xformOp:scale")
            .or_else(|| get_attribute_as_vec3(&mut reader, &sdf_path, "scale")) 
        {
            transform.scale = v;
        }
        if let Some(v) = get_attribute_as_vec3(&mut reader, &sdf_path, "xformOp:translate")
            .or_else(|| get_attribute_as_vec3(&mut reader, &sdf_path, "translate"))
        {
            transform.translation = v;
        }
        if let Some(v) = get_attribute_as_vec3(&mut reader, &sdf_path, "xformOp:rotateXYZ")
            .or_else(|| get_attribute_as_vec3(&mut reader, &sdf_path, "rotate"))
        {
            transform.rotation = Quat::from_euler(EulerRot::XYZ, v.x.to_radians(), v.y.to_radians(), v.z.to_radians()) * base_rotation;
        } else {
            transform.rotation = base_rotation;
        }

        if existing_vis.is_none() {
            commands.entity(entity).insert(Visibility::Inherited);
        }
        
        commands.entity(entity).insert((
            transform, 
            UsdVisualSynced, 
            InheritedVisibility::default(),
            ViewVisibility::default()
        ));

        // 4. Recursion
        for child_path in reader.get_name_children(&sdf_path) {
            let child_entity = commands.spawn((
                Name::new(child_path.to_string()),
                UsdPrimPath {
                    stage_handle: prim_path.stage_handle.clone(),
                    path: child_path.to_string(),
                },
                Transform::default(),
                Visibility::Inherited,
                InheritedVisibility::default(),
                ViewVisibility::default(),
            )).id();
            commands.entity(entity).add_child(child_entity);
        }
    }
}

fn get_attribute_as_vec3(reader: &mut TextReader, path: &SdfPath, attr: &str) -> Option<Vec3> {
    if let Some(v) = reader.get_prim_attribute_value::<Vec<f32>>(path, attr) {
        if v.len() >= 3 { return Some(Vec3::new(v[0], v[1], v[2])); }
    }
    if let Some(v) = reader.get_prim_attribute_value::<Vec<f64>>(path, attr) {
        if v.len() >= 3 { return Some(Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32)); }
    }
    None
}
