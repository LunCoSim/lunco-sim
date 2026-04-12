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
mod solar_panel_material;
pub use solar_panel_material::{SolarPanelExtension, SolarPanelMaterial, SolarPanelShaderPlugin};
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
            .add_plugins(SolarPanelShaderPlugin)
            .add_plugins(MaterialPlugin::<SolarPanelMaterial>::default())
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
/// # Material Handling
///
/// If the prim has `lunco:materialType = "solar_panel"`, the system creates a
/// `SolarPanelMaterial` with custom shader parameters read from USD attributes.
/// Otherwise, it uses a standard `StandardMaterial` with `primvars:displayColor`.
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
    mut solar_panel_materials: ResMut<Assets<SolarPanelMaterial>>,
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
        // Use explicit mesh builders to ensure mesh creation succeeds.
        let mesh_handle: Option<Handle<Mesh>> = match prim_type.as_deref() {
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

        if let Some(ref m) = mesh_handle {
            // PanelSurface: use SolarPanelMaterial custom shader
            // Frame and other prims: standard PBR material with USD color
            if prim_path.path.contains("PanelSurface") {
                let solar_mat = create_solar_panel_material(&reader, &sdf_path, &mut solar_panel_materials);
                commands.entity(entity).insert((
                    Mesh3d(m.clone()),
                    MeshMaterial3d(solar_mat),
                ));
            } else {
                apply_standard_material(&reader, &sdf_path, m, &mut materials, &mut commands.entity(entity));
            }
        }

        // Transform (position and rotation)
        // Preserve any existing transform set by the spawning code (e.g., rover position).
        // Only override position/rotation if the USD prim has explicit NON-ZERO values.
        // A zero translation in USD means "no offset" — it shouldn't overwrite a spawn position.
        let mut transform = existing_tf.cloned().unwrap_or_default();
        if let Some(v) = get_attribute_as_vec3(&reader, &sdf_path, "xformOp:translate") {
            // Only apply USD translation if it's non-zero (to avoid overwriting spawn positions)
            if v.length_squared() > 1e-6 {
                transform.translation = v;
            }
        }
        if let Some(v) = get_attribute_as_vec3(&reader, &sdf_path, "xformOp:rotateXYZ") {
            // USD stores rotation in degrees; convert to radians for Bevy
            // Only apply USD rotation if it's non-zero (to preserve existing spawn rotation)
            let is_zero = v.x.abs() < 1e-6 && v.y.abs() < 1e-6 && v.z.abs() < 1e-6;
            if !is_zero {
                let rx = v.x.to_radians();
                let ry = v.y.to_radians();
                let rz = v.z.to_radians();
                transform.rotation = Quat::from_euler(EulerRot::XYZ, rx, ry, rz);
            }
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
                GlobalTransform::default(),
                Visibility::Visible,
                InheritedVisibility::VISIBLE,
                ViewVisibility::default(),
            )).id();
            commands.entity(entity).add_child(child_entity);
        }
    }
}

/// Creates a SolarPanelMaterial from USD prim attributes.
fn create_solar_panel_material(
    reader: &TextReader,
    sdf_path: &SdfPath,
    materials: &mut ResMut<Assets<SolarPanelMaterial>>,
) -> Handle<SolarPanelMaterial> {
    let mut extension = SolarPanelExtension::default();

    if let Some(w) = reader.prim_attribute_value::<f64>(sdf_path, "width") {
        extension.panel_half_width = (w / 2.0) as f32;
    }
    if let Some(d) = reader.prim_attribute_value::<f64>(sdf_path, "depth") {
        extension.panel_half_depth = (d / 2.0) as f32;
    }

    if let Some(rows) = reader.prim_attribute_value::<i32>(sdf_path, "lunco:cellRows") {
        extension.cell_rows = rows as f32;
    } else if let Some(rows) = reader.prim_attribute_value::<f64>(sdf_path, "lunco:cellRows") {
        extension.cell_rows = rows as f32;
    }
    if let Some(cols) = reader.prim_attribute_value::<i32>(sdf_path, "lunco:cellCols") {
        extension.cell_cols = cols as f32;
    } else if let Some(cols) = reader.prim_attribute_value::<f64>(sdf_path, "lunco:cellCols") {
        extension.cell_cols = cols as f32;
    }

    if let Some(c) = get_attribute_as_vec3(reader, sdf_path, "lunco:cellColor") {
        extension.cell_color = LinearRgba::new(c.x, c.y, c.z, 1.0);
    }
    if let Some(c) = get_attribute_as_vec3(reader, sdf_path, "lunco:busLineColor") {
        extension.bus_line_color = LinearRgba::new(c.x, c.y, c.z, 1.0);
    }
    if let Some(c) = get_attribute_as_vec3(reader, sdf_path, "lunco:frameBorderColor") {
        extension.frame_border_color = LinearRgba::new(c.x, c.y, c.z, 1.0);
    }

    if let Some(v) = reader.prim_attribute_value::<f32>(sdf_path, "lunco:cellGap") {
        extension.cell_gap = v;
    } else if let Some(v) = reader.prim_attribute_value::<f64>(sdf_path, "lunco:cellGap") {
        extension.cell_gap = v as f32;
    }
    if let Some(v) = reader.prim_attribute_value::<f32>(sdf_path, "lunco:busLineWidth") {
        extension.bus_line_width = v;
    } else if let Some(v) = reader.prim_attribute_value::<f64>(sdf_path, "lunco:busLineWidth") {
        extension.bus_line_width = v as f32;
    }
    if let Some(v) = reader.prim_attribute_value::<f32>(sdf_path, "lunco:frameBorderWidth") {
        extension.frame_border_width = v;
    } else if let Some(v) = reader.prim_attribute_value::<f64>(sdf_path, "lunco:frameBorderWidth") {
        extension.frame_border_width = v as f32;
    }

    if let Some(v) = reader.prim_attribute_value::<f32>(sdf_path, "lunco:glassReflectivity") {
        extension.glass_reflectivity = v;
    } else if let Some(v) = reader.prim_attribute_value::<f64>(sdf_path, "lunco:glassReflectivity") {
        extension.glass_reflectivity = v as f32;
    }
    if let Some(v) = reader.prim_attribute_value::<f32>(sdf_path, "lunco:glassRoughness") {
        extension.glass_roughness = v;
    } else if let Some(v) = reader.prim_attribute_value::<f64>(sdf_path, "lunco:glassRoughness") {
        extension.glass_roughness = v as f32;
    }
    if let Some(v) = reader.prim_attribute_value::<f32>(sdf_path, "lunco:specularIntensity") {
        extension.specular_intensity = v;
    } else if let Some(v) = reader.prim_attribute_value::<f64>(sdf_path, "lunco:specularIntensity") {
        extension.specular_intensity = v as f32;
    }

    materials.add(SolarPanelMaterial {
        base: StandardMaterial {
            base_color: Color::LinearRgba(extension.cell_color),
            ..default()
        },
        extension,
    })
}

/// Applies a standard PBR material to an entity, using USD prim attributes.
fn apply_standard_material(
    reader: &TextReader,
    sdf_path: &SdfPath,
    mesh_handle: &Handle<Mesh>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    entity_cmd: &mut EntityCommands,
) {
    // Get color from primvars:displayColor attribute
    let color = get_attribute_as_vec3(reader, sdf_path, "primvars:displayColor")
        .map(|v| Color::srgb(v.x, v.y, v.z))
        .unwrap_or(Color::WHITE);

    entity_cmd.insert((
        Mesh3d(mesh_handle.clone()),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: color,
            ..default()
        }))
    ));
}

/// Reads a 3-component vector attribute from a USD prim.
///
/// Handles all common USD vector types:
/// - `color3f` → `Value::Vec3f`
/// - `double3` → `Value::Vec3d`
/// - `float3` → `Value::Vec3f`
/// - `Vec<f32>` / `Vec<f64>` array forms
///
/// Returns `None` if the attribute doesn't exist or can't be converted.
fn get_attribute_as_vec3(reader: &TextReader, path: &SdfPath, attr: &str) -> Option<Vec3> {
    // Handle fixed-size array types first (Vec3f, Vec3d)
    if let Some(v) = reader.prim_attribute_value::<[f32; 3]>(path, attr) {
        return Some(Vec3::new(v[0], v[1], v[2]));
    }
    if let Some(v) = reader.prim_attribute_value::<[f64; 3]>(path, attr) {
        return Some(Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32));
    }
    // Handle Vec forms as fallback
    if let Some(v) = reader.prim_attribute_value::<Vec<f32>>(path, attr) {
        if v.len() >= 3 { return Some(Vec3::new(v[0], v[1], v[2])); }
    }
    if let Some(v) = reader.prim_attribute_value::<Vec<f64>>(path, attr) {
        if v.len() >= 3 { return Some(Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32)); }
    }
    None
}
