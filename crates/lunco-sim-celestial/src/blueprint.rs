use bevy::prelude::*;
use bevy::pbr::{MaterialExtension, ExtendedMaterial};
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone, Copy)]
pub struct BlueprintExtension {
    #[uniform(100)]
    pub line_color: LinearRgba,
    #[uniform(100)]
    pub grid_scale: f32,
    #[uniform(100)]
    pub line_width: f32,
    #[uniform(100)]
    pub transition: f32, // 0.0 = Lat/Long (High), 1.0 = Blueprint (Low)
    #[uniform(100)]
    pub moon_radius: f32,
}

impl Default for BlueprintExtension {
    fn default() -> Self {
        Self {
            line_color: LinearRgba::new(0.0, 0.5, 1.0, 1.0),
            grid_scale: 10.0,
            line_width: 2.0,
            transition: 0.0,
            moon_radius: 1737_000.0,
        }
    }
}

impl MaterialExtension for BlueprintExtension {
    fn fragment_shader() -> ShaderRef {
        "shaders/blueprint_extension.wgsl".into()
    }
}

pub type BlueprintMaterial = ExtendedMaterial<StandardMaterial, BlueprintExtension>;
