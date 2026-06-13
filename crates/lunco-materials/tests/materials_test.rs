//! Unit tests for lunco-materials crate.

use lunco_materials::{ShaderMaterial, BlueprintExtension};

/// Verifies the general ShaderMaterial has sensible default uniforms.
/// (The solar panel is now driven by this material + `shaders/solar_panel.wgsl`,
/// not a bespoke `SolarPanelExtension`.)
#[test]
fn test_shader_material_defaults() {
    let m = ShaderMaterial::default();
    // Default colour (legacy `colorA`) is non-zero, and the opaque uniform
    // block reflects it at lane 0 (colorA @ byte 0).
    let c = m.get_color("colorA").expect("colorA default");
    assert!(c[0] > 0.0 || c[1] > 0.0 || c[2] > 0.0);
    assert_eq!(m.raw[0].x, c[0]);
    // Engine.x (sun visibility) defaults lit so un-shaded props aren't black.
    assert_eq!(m.raw[5].x, 1.0); // engine @ byte 80 → vec4 lane 5
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
