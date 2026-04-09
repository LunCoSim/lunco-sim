//! # Solar Panel Material
//!
//! Custom PBR material extension for rendering realistic photovoltaic solar panels.
//!
//! ## Architecture
//! - Shader: `assets/shaders/solar_panel_extension.wgsl`
//! - Parameters: Defined in USD prim attributes (e.g., `lunco:cellRows`, `lunco:cellColor`)
//! - Material type: `SolarPanelMaterial` = `ExtendedMaterial<StandardMaterial, SolarPanelExtension>`
//!
//! ## How It Works
//! 1. USD file defines panel geometry and shader parameters as prim attributes
//! 2. USD loader reads attributes and creates `SolarPanelMaterial`
//! 3. Shader renders cell grid, bus lines, glass reflections procedurally
//!
//! ## Tunability
//! All parameters are exposed as uniforms — change USD attributes, get different visuals.
//! No code changes needed.

use bevy::prelude::*;
use bevy::pbr::{MaterialExtension, ExtendedMaterial};
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

/// Material extension for solar panel cell grid rendering.
///
/// This extension adds a realistic photovoltaic cell pattern on top of the
/// base PBR material. Parameters control the cell grid layout, bus line
/// appearance, glass reflectivity, and frame border.
///
/// # USD Integration
///
/// All fields are populated from USD prim attributes by the USD Bevy loader.
/// The loader reads `lunco:cellRows`, `lunco:cellColor`, etc. and creates
/// this extension automatically.
///
/// # Tunability
///
/// All parameters are exposed as uniforms and can be changed at runtime by
/// modifying the USD file. The shader reads these values every frame.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone, Copy)]
pub struct SolarPanelExtension {
    /// Half-width of the panel in meters (panel_width / 2). Used to anchor
    /// the cell grid to the panel edges.
    #[uniform(100)]
    pub panel_half_width: f32,

    /// Half-depth of the panel in meters (panel_depth / 2).
    #[uniform(100)]
    pub panel_half_depth: f32,

    /// Number of solar cells along the panel width (X axis).
    #[uniform(100)]
    pub cell_rows: f32,

    /// Number of solar cells along the panel depth (Z axis).
    #[uniform(100)]
    pub cell_cols: f32,

    /// Color of the individual silicon cells (default: deep blue).
    #[uniform(100)]
    pub cell_color: LinearRgba,

    /// Color of the metallic bus lines between cells (default: silver).
    #[uniform(100)]
    pub bus_line_color: LinearRgba,

    /// Color of the frame border visible around the cell array.
    #[uniform(100)]
    pub frame_border_color: LinearRgba,

    /// Gap between adjacent cells in meters (default: 0.02m = 2cm).
    #[uniform(100)]
    pub cell_gap: f32,

    /// Width of bus lines in meters (default: 0.003m = 3mm).
    #[uniform(100)]
    pub bus_line_width: f32,

    /// Width of the frame border visible on the top surface in meters (default: 0.05m = 5cm).
    #[uniform(100)]
    pub frame_border_width: f32,

    /// Glass surface reflectivity (0.0 = matte, 1.0 = mirror).
    /// Controls how much environment light is reflected.
    #[uniform(100)]
    pub glass_reflectivity: f32,

    /// Glass surface roughness (0.0 = perfectly smooth, 1.0 = rough).
    /// Lower values create sharper specular highlights.
    #[uniform(100)]
    pub glass_roughness: f32,

    /// Intensity of specular highlights from light sources (default: 0.8).
    /// Simulates sun glare on the glass surface.
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
            cell_color: LinearRgba::new(0.05, 0.05, 0.35, 1.0),       // Deep blue
            bus_line_color: LinearRgba::new(0.85, 0.85, 0.90, 1.0),   // Silver
            frame_border_color: LinearRgba::new(0.35, 0.35, 0.38, 1.0), // Dark gray
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
        "shaders/solar_panel_extension.wgsl".into()
    }
}

/// Solar panel material type — PBR standard material extended with cell grid shader.
///
/// This is the material type used for solar panel surfaces. It combines
/// standard PBR lighting with a custom fragment shader that renders the
/// photovoltaic cell grid pattern.
///
/// # Example
///
/// ```ignore
/// let solar_mat = materials.add(SolarPanelMaterial {
///     base: StandardMaterial {
///         base_color: Color::srgb(0.05, 0.05, 0.35),
///         ..default()
///     },
///     extension: SolarPanelExtension::default(),
/// });
/// ```
pub type SolarPanelMaterial = ExtendedMaterial<StandardMaterial, SolarPanelExtension>;
