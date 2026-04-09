use bevy::prelude::*;
use bevy::pbr::{MaterialExtension, ExtendedMaterial};
use bevy::render::render_resource::AsBindGroup;
use bevy_shader::Shader;
use bevy_asset::load_internal_asset;

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
        load_internal_asset!(
            app,
            crate::embedded_assets::BLUEPRINT_SHADER_HANDLE,
            "../../../assets/shaders/blueprint_extension.wgsl",
            Shader::from_wgsl
        );
    }
}

/// Holds the compiled blueprint shader handle.
#[derive(Resource)]
pub struct BlueprintShaderHandle(pub Handle<Shader>);

impl MaterialExtension for BlueprintExtension {
    fn fragment_shader() -> bevy::shader::ShaderRef {
        // Returns the handle to our embedded shader (set by EmbeddedAssetsPlugin on wasm32)
        crate::embedded_assets::BLUEPRINT_SHADER_HANDLE.clone().into()
    }
}

pub type BlueprintMaterial = ExtendedMaterial<StandardMaterial, BlueprintExtension>;
