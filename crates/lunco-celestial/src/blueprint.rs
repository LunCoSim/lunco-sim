use bevy::prelude::*;
use bevy::pbr::{MaterialExtension, ExtendedMaterial};
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone, Copy)]
pub struct BlueprintExtension {
    #[uniform(100)]
    pub high_color: LinearRgba,
    #[uniform(100)]
    pub low_color: LinearRgba,
    #[uniform(100)]
    pub high_line_color: LinearRgba,
    #[uniform(100)]
    pub low_line_color: LinearRgba,
    #[uniform(100)]
    pub subdivisions: Vec2, // x=longitude, y=latitude
    #[uniform(100)]
    pub fade_range: Vec2,   // min, max fwidth for fade-out
    #[uniform(100)]
    pub grid_scale: f32,
    #[uniform(100)]
    pub line_width: f32,
    #[uniform(100)]
    pub transition: f32,
    #[uniform(100)]
    pub body_radius: f32,

    /// Major grid spacing in meters (default: 1.0 m).
    #[uniform(100)]
    pub major_grid_spacing: f32,
    /// Minor grid spacing in meters (default: 0.1 m = 10 cm).
    #[uniform(100)]
    pub minor_grid_spacing: f32,
    /// Width of major grid lines (default: 2.0, bold).
    #[uniform(100)]
    pub major_line_width: f32,
    /// Width of minor grid lines (default: 0.5, barely visible).
    #[uniform(100)]
    pub minor_line_width: f32,
    /// Alpha/color multiplier for minor grid lines (default: 0.2, faint).
    #[uniform(100)]
    pub minor_line_fade: f32,
    /// Base surface color (default: dark grey for ground, different for ramp).
    #[uniform(100)]
    pub surface_color: LinearRgba,
}

impl Default for BlueprintExtension {
    fn default() -> Self {
        Self {
            high_color: LinearRgba::WHITE,
            low_color: LinearRgba::WHITE,
            high_line_color: LinearRgba::new(1.0, 1.0, 1.0, 1.0),
            low_line_color: LinearRgba::new(1.0, 1.0, 1.0, 1.0),
            subdivisions: Vec2::new(24.0, 12.0),
            fade_range: Vec2::new(0.2, 0.6),
            grid_scale: 10.0,
            line_width: 2.0,
            transition: 0.0,
            body_radius: 1737_000.0,
            major_grid_spacing: 1.0,
            minor_grid_spacing: 0.5,
            major_line_width: 0.75,
            minor_line_width: 0.4,
            minor_line_fade: 0.3,
            surface_color: LinearRgba::new(0.2, 0.2, 0.2, 1.0),
        }
    }
}

impl MaterialExtension for BlueprintExtension {
    fn fragment_shader() -> ShaderRef {
        "shaders/blueprint_extension.wgsl".into()
    }
}

pub type BlueprintMaterial = ExtendedMaterial<StandardMaterial, BlueprintExtension>;
