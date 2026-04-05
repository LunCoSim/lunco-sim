use bevy::prelude::*;
use bevy::asset::{AssetLoader, LoadContext, io::Reader};
use openusd::usda::TextReader;
use openusd::sdf::{AbstractData, Path as SdfPath, Value};
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
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        use futures_lite::AsyncReadExt;
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let data = String::from_utf8(bytes)?;

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
        let Some(stage) = stages.get(&prim_handle_to_stage(&prim_path.stage_handle)) else { continue; };
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { continue; };

        // HACK: Clone reader for thread-safety in this frame
        let mut reader = (*stage.reader).clone();

        // 1. DATA-DRIVEN Visual Mesh based on USD typeName
        let mut mesh_handle = None;
        if let Ok(val) = reader.get(&sdf_path, "typeName") {
            if let Value::Token(ty) = &*val {
                match ty.as_str() {
                    "Cube" => mesh_handle = Some(meshes.add(Cuboid::default())),
                    "Sphere" => mesh_handle = Some(meshes.add(Sphere::new(1.0).mesh().ico(32).unwrap())),
                    "Cylinder" => mesh_handle = Some(meshes.add(Cylinder::default())),
                    _ => {
                        debug!("Prim {} has unknown typeName: {}", prim_path.path, ty);
                    }
                }
            }
        }

        // 2. Map Color from displayColor (Type-safe)
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

        // 3. Map Transform (Type-safe)
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
        info!("Successfully LOADED visuals from USD for {}", prim_path.path);

        // 4. GENERIC RECURSION
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

// Internal helper
fn prim_handle_to_stage(handle: &Handle<UsdStageAsset>) -> AssetId<UsdStageAsset> {
    handle.id()
}
