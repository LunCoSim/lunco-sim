use bevy::prelude::*;
use bevy::asset::{AssetLoader, LoadContext, io::Reader};
use openusd::usda::TextReader;
use openusd::sdf::{AbstractData, Path as SdfPath, Value};
use std::sync::Arc;
use futures_lite::AsyncReadExt;

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

        // Note: We'd ideally want TextReader::from_string in the fork, 
        // but for now we can simulate it if needed or add it to fork.
        // Actually, let's just use the existing fork's parser directly if possible.
        
        // For this version, we use a small hack to use the existing parser
        let mut parser = openusd::usda::parser::Parser::new(&data);
        let data_map = parser.parse().map_err(|e| anyhow::anyhow!("USD Parse Error: {}", e))?;
        let reader = TextReader::from_data(data_map);

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

/// Resource that maps Stage Entities to their Handles for lookup
#[derive(Component)]
pub struct UsdStageResource {
    pub handle: Handle<UsdStageAsset>,
}

#[derive(Component)]
pub struct UsdVisualSynced;

fn sync_usd_visuals(
    mut commands: Commands,
    query: Query<(Entity, &UsdPrimPath), Without<UsdVisualSynced>>,
    stages: Res<Assets<UsdStageAsset>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, prim_path) in query.iter() {
        let Some(stage) = stages.get(&prim_path.stage_handle) else { continue; };
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { continue; };

        // 1. Detect Visual Mesh based on USD TypeName
        let mut mesh_handle = None;
        
        // We need a way to get the type name from the asset
        // For now we'll use our existing logic, but we need to bypass the &mut requirement of openusd 0.1.4
        // I'll add a 'get_const' or similar to the fork later, for now we do a local clone or fix.
        
        // HACK: TextReader::get currently requires &mut self because of internal caching
        // We'll use a local mut clone for this frame until we fix the fork to be thread-safe.
        let mut reader = (*stage.reader).clone();

        if let Ok(val) = reader.get(&sdf_path, "typeName") {
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
        if let Some(v) = reader.get_prim_attribute_value::<Vec<f32>>(&sdf_path, "primvars:displayColor") {
            if v.len() >= 3 {
                color = Color::srgb(v[0], v[1], v[2]);
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
        if let Some(v) = reader.get_prim_attribute_value::<Vec<f32>>(&sdf_path, "xformOp:scale")
            .or_else(|| reader.get_prim_attribute_value::<Vec<f32>>(&sdf_path, "scale")) 
        {
            if v.len() >= 3 { transform.scale = Vec3::new(v[0], v[1], v[2]); }
        }
        if let Some(v) = reader.get_prim_attribute_value::<Vec<f32>>(&sdf_path, "xformOp:translate")
            .or_else(|| reader.get_prim_attribute_value::<Vec<f32>>(&sdf_path, "translate"))
        {
            if v.len() >= 3 { transform.translation = Vec3::new(v[0], v[1], v[2]); }
        }

        commands.entity(entity).insert((transform, UsdVisualSynced));

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
            )).id();
            commands.entity(entity).add_child(child_entity);
        }
    }
}
