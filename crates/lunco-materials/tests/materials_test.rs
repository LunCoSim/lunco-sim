//! Unit tests for lunco-materials crate.

use lunco_materials::{ShaderMaterial, BlueprintExtension};

/// Verifies the general ShaderMaterial has sensible default uniforms.
/// (The solar panel is now driven by this material + `shaders/solar_panel.wgsl`,
/// not a bespoke `SolarPanelExtension`.)
#[test]
fn test_shader_material_defaults() {
    let m = ShaderMaterial::default();
    // Default colors are non-zero and params start cleared.
    assert!(m.color_a.red > 0.0 || m.color_a.green > 0.0 || m.color_a.blue > 0.0);
    assert_eq!(m.params, bevy::math::Vec4::ZERO);
    assert_eq!(m.params2, bevy::math::Vec4::ZERO);
}

/// Verifies BlueprintExtension has sensible default values
#[test]
fn test_blueprint_extension_defaults() {
    let ext = BlueprintExtension::default();

    // Grid parameters
    assert_eq!(ext.major_grid_spacing, 1.0);
    assert_eq!(ext.minor_grid_spacing, 0.5);
    assert_eq!(ext.major_line_width, 0.75);
    assert_eq!(ext.minor_line_width, 0.4);
    assert_eq!(ext.minor_line_fade, 0.3);

    // Surface color is non-white
    assert!(ext.surface_color.red < 0.5);
}
