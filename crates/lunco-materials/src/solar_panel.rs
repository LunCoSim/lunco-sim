//! Solar Panel Material
//!
//! Custom PBR material extension for rendering realistic photovoltaic solar panels.
//!
//! ## Architecture
//! - Shader: `assets/shaders/solar_panel_extension.wgsl`
//! - Parameters: Defined in USD prim attributes via `primvars:` namespace
//! - Material type: `SolarPanelMaterial` = `ExtendedMaterial<StandardMaterial, SolarPanelExtension>`
//!
//! ## How It Works
//! 1. USD file defines panel geometry and shader parameters as prim attributes
//! 2. `SolarPanelMaterialPlugin` post-sync system detects `primvars:materialType = "solar_panel"`
//! 3. Material is created from USD primvars and assigned to the entity
//! 4. Shader renders cell grid, bus lines, glass reflections procedurally

use bevy::prelude::*;
use bevy::pbr::{MaterialExtension, MaterialPlugin, ExtendedMaterial};
use bevy::render::render_resource::AsBindGroup;
use bevy::asset::load_internal_asset;
use bevy::shader::{Shader, ShaderRef};
use std::marker::PhantomData;
use uuid::Uuid;

use lunco_usd_bevy::UsdPrimPath;
use openusd::usda::TextReader;
use openusd::sdf::Path as SdfPath;
use crate::get_attribute_as_vec3;

/// UUID for the solar panel shader.
const SOLAR_PANEL_SHADER_UUID: Uuid = Uuid::from_u128(0x9a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d);
pub const SOLAR_PANEL_SHADER_HANDLE: Handle<Shader> = Handle::Uuid(SOLAR_PANEL_SHADER_UUID, PhantomData);

/// Solar panel extension uniforms — matches SolarPanelExtension Rust struct.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone, Copy)]
pub struct SolarPanelExtension {
    #[uniform(100)]
    pub panel_half_width: f32,
    #[uniform(100)]
    pub panel_half_depth: f32,
    #[uniform(100)]
    pub cell_rows: f32,
    #[uniform(100)]
    pub cell_cols: f32,
    #[uniform(100)]
    pub cell_color: LinearRgba,
    #[uniform(100)]
    pub bus_line_color: LinearRgba,
    #[uniform(100)]
    pub frame_border_color: LinearRgba,
    #[uniform(100)]
    pub cell_gap: f32,
    #[uniform(100)]
    pub bus_line_width: f32,
    #[uniform(100)]
    pub frame_border_width: f32,
    #[uniform(100)]
    pub glass_reflectivity: f32,
    #[uniform(100)]
    pub glass_roughness: f32,
    #[uniform(100)]
    pub specular_intensity: f32,
}

impl Default for SolarPanelExtension {
    fn default() -> Self {
        Self {
            panel_half_width: 3.0,
            panel_half_depth: 1.5,
            cell_rows: 12.0,
            cell_cols: 6.0,
            cell_color: LinearRgba::new(0.05, 0.05, 0.35, 1.0),
            bus_line_color: LinearRgba::new(0.85, 0.85, 0.90, 1.0),
            frame_border_color: LinearRgba::new(0.35, 0.35, 0.38, 1.0),
            cell_gap: 0.02,
            bus_line_width: 0.003,
            frame_border_width: 0.05,
            glass_reflectivity: 0.15,
            glass_roughness: 0.05,
            specular_intensity: 0.8,
        }
    }
}

impl MaterialExtension for SolarPanelExtension {
    fn fragment_shader() -> ShaderRef {
        SOLAR_PANEL_SHADER_HANDLE.into()
    }
}

/// Plugin that registers the solar panel shader and material.
///
/// This embeds the WGSL shader at compile time and registers a post-sync
/// system that applies SolarPanelMaterial to entities with
/// `primvars:materialType = "solar_panel"`.
pub struct SolarPanelMaterialPlugin;

impl Plugin for SolarPanelMaterialPlugin {
    fn build(&self, app: &mut App) {
        load_internal_asset!(
            app,
            SOLAR_PANEL_SHADER_HANDLE,
            "../../../assets/shaders/solar_panel_extension.wgsl",
            Shader::from_wgsl
        );
        app.add_plugins(MaterialPlugin::<SolarPanelMaterial>::default());
        app.add_systems(Update, apply_solar_panel_material.after(lunco_usd_bevy::sync_usd_visuals));
    }
}

/// Solar panel material type.
pub type SolarPanelMaterial = ExtendedMaterial<StandardMaterial, SolarPanelExtension>;

/// Marker component preventing re-processing of BlueprintMaterial assignment.
#[derive(Component)]
pub struct SolarPanelMaterialApplied;

/// Post-sync system that applies SolarPanelMaterial to matching USD entities.
pub fn apply_solar_panel_material(
    mut commands: Commands,
    stages: Res<Assets<lunco_usd_bevy::UsdStageAsset>>,
    mut materials: ResMut<Assets<SolarPanelMaterial>>,
    q_all: Query<(Entity, &UsdPrimPath), (With<Mesh3d>, Without<SolarPanelMaterialApplied>)>,
) {
    for (entity, prim_path) in q_all.iter() {
        let Some(stage) = stages.get(&prim_path.stage_handle) else { continue };
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { continue };
        let reader = (*stage.reader).clone();

        let mat_type: Option<String> = reader.prim_attribute_value(&sdf_path, "primvars:materialType");
        if mat_type.as_deref() != Some("solar_panel") { continue; }

        let solar_mat = create_solar_panel_material(&reader, &sdf_path, &mut materials);
        commands.entity(entity).insert((
            MeshMaterial3d(solar_mat),
            SolarPanelMaterialApplied,
        ));
    }
}

/// Creates a SolarPanelMaterial from USD primvars attributes.
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

    if let Some(rows) = reader.prim_attribute_value::<i32>(sdf_path, "primvars:cellRows") {
        extension.cell_rows = rows as f32;
    } else if let Some(rows) = reader.prim_attribute_value::<f64>(sdf_path, "primvars:cellRows") {
        extension.cell_rows = rows as f32;
    }
    if let Some(cols) = reader.prim_attribute_value::<i32>(sdf_path, "primvars:cellCols") {
        extension.cell_cols = cols as f32;
    } else if let Some(cols) = reader.prim_attribute_value::<f64>(sdf_path, "primvars:cellCols") {
        extension.cell_cols = cols as f32;
    }

    if let Some(c) = get_attribute_as_vec3(reader, sdf_path, "primvars:cellColor") {
        extension.cell_color = LinearRgba::new(c.x, c.y, c.z, 1.0);
    }
    if let Some(c) = get_attribute_as_vec3(reader, sdf_path, "primvars:busLineColor") {
        extension.bus_line_color = LinearRgba::new(c.x, c.y, c.z, 1.0);
    }
    if let Some(c) = get_attribute_as_vec3(reader, sdf_path, "primvars:frameBorderColor") {
        extension.frame_border_color = LinearRgba::new(c.x, c.y, c.z, 1.0);
    }

    if let Some(v) = reader.prim_attribute_value::<f32>(sdf_path, "primvars:cellGap") {
        extension.cell_gap = v;
    } else if let Some(v) = reader.prim_attribute_value::<f64>(sdf_path, "primvars:cellGap") {
        extension.cell_gap = v as f32;
    }
    if let Some(v) = reader.prim_attribute_value::<f32>(sdf_path, "primvars:busLineWidth") {
        extension.bus_line_width = v;
    } else if let Some(v) = reader.prim_attribute_value::<f64>(sdf_path, "primvars:busLineWidth") {
        extension.bus_line_width = v as f32;
    }
    if let Some(v) = reader.prim_attribute_value::<f32>(sdf_path, "primvars:frameBorderWidth") {
        extension.frame_border_width = v;
    } else if let Some(v) = reader.prim_attribute_value::<f64>(sdf_path, "primvars:frameBorderWidth") {
        extension.frame_border_width = v as f32;
    }

    if let Some(v) = reader.prim_attribute_value::<f32>(sdf_path, "primvars:glassReflectivity") {
        extension.glass_reflectivity = v;
    } else if let Some(v) = reader.prim_attribute_value::<f64>(sdf_path, "primvars:glassReflectivity") {
        extension.glass_reflectivity = v as f32;
    }
    if let Some(v) = reader.prim_attribute_value::<f32>(sdf_path, "primvars:glassRoughness") {
        extension.glass_roughness = v;
    } else if let Some(v) = reader.prim_attribute_value::<f64>(sdf_path, "primvars:glassRoughness") {
        extension.glass_roughness = v as f32;
    }
    if let Some(v) = reader.prim_attribute_value::<f32>(sdf_path, "primvars:specularIntensity") {
        extension.specular_intensity = v;
    } else if let Some(v) = reader.prim_attribute_value::<f64>(sdf_path, "primvars:specularIntensity") {
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
