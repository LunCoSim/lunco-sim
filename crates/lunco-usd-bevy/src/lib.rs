//! # LunCoSim USD → Bevy Visual Sync
//!
//! Responsible for spawning child entities for USD prims and attaching visual components
//! (meshes, materials, transforms). This is the **first** plugin in the USD processing
//! pipeline — it must run before the Avian physics and Sim simulation plugins.
//!
//! ## How It Works
//!
//! 1. The asset loader (`UsdLoader`) reads a `.usda` file, parses it, and resolves all
//!    external references (e.g., wheel component files) via `UsdComposer::flatten()`.
//! 2. The `sync_usd_visuals` system iterates over all entities with `UsdPrimPath` that
//!    haven't been processed yet (`Without<UsdVisualSynced>`).
//! 3. For each prim, it creates a mesh based on the prim type (`Cube`, `Cylinder`, `Sphere`)
//!    using explicit dimensions from the USD file.
//! 4. It spawns child entities for each prim child, pre-populating their transforms so
//!    physics systems see them in the correct positions.
//!
//! ## Coordinate Systems
//!
//! USD uses Y-up, +Z-forward. Bevy uses Y-up, -Z-forward. The USD files store rotation
//! in degrees via `xformOp:rotateXYZ`. This system converts them to radians and applies
//! them as Bevy quaternions.
//!
//! ## Mesh Dimensions
//!
//! Bevy's `Cuboid::new()` and `Collider::cuboid()` take **full dimensions**, not
//! half-extents. The USD files store full dimensions (`width`, `height`, `depth`),
//! so no scaling is needed.
//!
//! ## Why Not Use the Observer?
//!
//! The `On<Add, UsdPrimPath>` observer fires when the entity is spawned, but the USD
//! asset may not be loaded yet (async loading). The `sync_usd_visuals` system runs in
//! the `Update` schedule and retries every frame until the asset is available, then
//! marks the entity with `UsdVisualSynced` to prevent re-processing.

use bevy::prelude::*;
use bevy::asset::{AssetLoader, LoadContext, io::Reader};
use openusd::usda::TextReader;
use openusd::sdf::{AbstractData, Path as SdfPath, Value};
use lunco_usd_composer::UsdComposer;
use big_space::prelude::CellCoord;
use std::sync::Arc;

/// Bevy plugin for USD visual synchronization.
///
/// Registers the `UsdStageAsset` type, the USD asset loader, and the `sync_usd_visuals`
/// system that processes USD prims into Bevy entities with meshes and transforms.
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
///
/// Contains a flattened USD reader with all external references resolved.
/// Created by the `UsdLoader` asset loader when a `.usda` file is loaded.
#[derive(Asset, TypePath, Clone)]
pub struct UsdStageAsset {
    /// Flattened USD reader with all references resolved.
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
        // Read raw bytes from the .usda file
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let data = String::from_utf8(bytes)?;

        // Parse the USD text format
        let mut parser = openusd::usda::parser::Parser::new(&data);
        let data_map = parser.parse().map_err(|e| anyhow::anyhow!("USD Parse Error: {}", e))?;
        let reader = TextReader::from_data(data_map);

        // Resolve external references (e.g., @/components/mobility/wheel.usda@)
        // The composer walks the directory tree to find the assets/ root and resolves
        // "/"-prefixed paths against it.
        let reader = if let Some(parent) = load_context.path().path().parent() {
            let asset_root = std::path::Path::new("assets");
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

/// Marks an entity as representing a USD prim path.
///
/// This component is added to every entity that corresponds to a USD prim. The system
/// uses it to look up the prim's attributes from the loaded USD stage.
///
/// # Fields
/// - `stage_handle`: Handle to the loaded `UsdStageAsset`
/// - `path`: USD prim path (e.g., `/SandboxRover` or `/SandboxRover/Wheel_FL`)
#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
pub struct UsdPrimPath {
    /// Handle to the loaded USD stage asset.
    pub stage_handle: Handle<UsdStageAsset>,
    /// USD prim path within the stage (e.g., `/SandboxRover/Wheel_FL`).
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

/// Marker component indicating that an entity has been processed by `sync_usd_visuals`.
///
/// Prevents the system from re-processing the same entity on subsequent frames.
#[derive(Component)]
pub struct UsdVisualSynced;

/// System that synchronizes USD prims into Bevy entities with visual components.
///
/// For each entity with `UsdPrimPath` (but not `UsdVisualSynced`):
/// 1. Looks up the prim's attributes from the loaded USD stage
/// 2. Creates a mesh based on prim type (Cube, Cylinder, Sphere) with explicit dimensions
/// 3. Applies the prim's transform (position + rotation)
/// 4. Spawns child entities for each prim child, pre-populating their transforms
/// 5. Marks the entity with `UsdVisualSynced` to prevent re-processing
///
/// # Important
///
/// This system runs in the `Update` schedule and retries every frame until the USD asset
/// is loaded. This is necessary because asset loading is asynchronous — the entity may
/// be spawned before the asset is ready.
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
                    continue;
                }
            }
        }

        // Get prim type (Cube, Cylinder, Sphere, etc.)
        let prim_type = if let Ok(val) = reader.get(&sdf_path, "typeName") {
            if let Value::Token(ty) = &*val {
                Some(ty.clone())
            } else {
                None
            }
        } else {
            None
        };

        // Create mesh based on prim type and explicit dimensions.
        // Bevy's Cuboid::new() takes FULL dimensions (not half-extents),
        // matching the USD file's width/height/depth attributes.
        let mesh_handle = match prim_type.as_deref() {
            Some("Cube") => {
                if let (Some(width), Some(height), Some(depth)) = (
                    reader.prim_attribute_value::<f64>(&sdf_path, "width"),
                    reader.prim_attribute_value::<f64>(&sdf_path, "height"),
                    reader.prim_attribute_value::<f64>(&sdf_path, "depth"),
                ) {
                    Some(meshes.add(Cuboid::new(width as f32, height as f32, depth as f32)))
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

        // Get color from primvars:displayColor attribute
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
        // Preserve any existing transform set by the spawning code (e.g., rover position).
        // Only override position/rotation if the USD prim has explicit values.
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

        // Spawn children with their transforms pre-populated so physics sees them correctly.
        // This is critical for wheel positions — they must be at the correct offsets from
        // the chassis center before the suspension system runs.
        for child_path in reader.prim_children(&sdf_path) {
            if let Ok(val) = reader.get(&child_path, "active") {
                if let Value::Bool(active) = &*val {
                    if !*active { continue; }
                }
            }

            // Pre-read child transform from USD
            let mut child_tf = Transform::default();
            if let Some(v) = get_attribute_as_vec3(&reader, &child_path, "xformOp:translate") {
                child_tf.translation = v;
            }
            if let Some(v) = get_attribute_as_vec3(&reader, &child_path, "xformOp:rotateXYZ") {
                let rx = v.x.to_radians();
                let ry = v.y.to_radians();
                let rz = v.z.to_radians();
                child_tf.rotation = Quat::from_euler(EulerRot::XYZ, rx, ry, rz);
            }

            let child_entity = commands.spawn((
                Name::new(child_path.to_string()),
                UsdPrimPath {
                    stage_handle: prim_path.stage_handle.clone(),
                    path: child_path.to_string(),
                },
                child_tf,
                CellCoord::default(),
                Visibility::Inherited,
                InheritedVisibility::default(),
                ViewVisibility::default(),
            )).id();
            commands.entity(entity).add_child(child_entity);
        }
    }
}

/// Reads a Vec3 attribute from a USD prim.
///
/// Tries both `Vec<f32>` and `Vec<f64>` since USD stores vector attributes as
/// floating-point arrays. Returns `None` if the attribute doesn't exist or has
/// fewer than 3 elements.
fn get_attribute_as_vec3(reader: &TextReader, path: &SdfPath, attr: &str) -> Option<Vec3> {
    if let Some(v) = reader.prim_attribute_value::<Vec<f32>>(path, attr) {
        if v.len() >= 3 { return Some(Vec3::new(v[0], v[1], v[2])); }
    }
    if let Some(v) = reader.prim_attribute_value::<Vec<f64>>(path, attr) {
        if v.len() >= 3 { return Some(Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32)); }
    }
    None
}
