use bevy::prelude::*;
use bevy::pbr::{MaterialExtension, ExtendedMaterial};
use bevy::render::render_resource::AsBindGroup;
use bevy_shader::Shader;

// Shader source embedded at compile time from root assets/shaders/
const BLUEPRINT_SHADER_SRC: &str = include_str!("../../../assets/shaders/blueprint_extension.wgsl");

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
    pub subdivisions: Vec2,
    #[uniform(100)]
    pub fade_range: Vec2,
    #[uniform(100)]
    pub grid_scale: f32,
    #[uniform(100)]
    pub line_width: f32,
    #[uniform(100)]
    pub transition: f32,
    #[uniform(100)]
    pub body_radius: f32,
    #[uniform(100)]
    pub major_grid_spacing: f32,
    #[uniform(100)]
    pub minor_grid_spacing: f32,
    #[uniform(100)]
    pub major_line_width: f32,
    #[uniform(100)]
    pub minor_line_width: f32,
    #[uniform(100)]
    pub minor_line_fade: f32,
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

/// Registers the embedded blueprint shader into the asset server.
/// MUST be added before `MaterialPlugin::<BlueprintMaterial>` so the shader
/// handle is available when materials are created.
pub struct BlueprintShaderPlugin;

impl Plugin for BlueprintShaderPlugin {
    fn build(&self, app: &mut App) {
        let mut shaders = app.world_mut().resource_mut::<Assets<Shader>>();
        let handle = shaders.add(Shader::from_wgsl(
            BLUEPRINT_SHADER_SRC,
            "shaders/blueprint_extension.wgsl",
        ));
        app.insert_resource(BlueprintShaderHandle(handle));
    }
}

/// Holds the compiled blueprint shader handle.
#[derive(Resource)]
pub struct BlueprintShaderHandle(pub Handle<Shader>);

impl MaterialExtension for BlueprintExtension {
    fn fragment_shader() -> bevy::shader::ShaderRef {
        // Returns the path key that matches our embedded shader registration
        "shaders/blueprint_extension.wgsl".into()
    }
}

pub type BlueprintMaterial = ExtendedMaterial<StandardMaterial, BlueprintExtension>;
