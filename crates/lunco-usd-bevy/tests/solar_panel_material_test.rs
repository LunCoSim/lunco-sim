//! Unit tests for lunco-usd-bevy crate.

use bevy::prelude::*;
use lunco_usd_bevy::{SolarPanelExtension, SolarPanelMaterial};

/// Verifies SolarPanelExtension has sensible default values
#[test]
fn test_solar_panel_extension_defaults() {
    let ext = SolarPanelExtension::default();

    // Panel dimensions
    assert_eq!(ext.panel_half_width, 3.0);
    assert_eq!(ext.panel_half_depth, 1.5);

    // Grid layout
    assert_eq!(ext.cell_rows, 12.0);
    assert_eq!(ext.cell_cols, 6.0);

    // Colors are non-default (should be deep blue — blue channel > red channel)
    assert!(ext.cell_color.blue > ext.cell_color.red);

    // Dimensional parameters
    assert_eq!(ext.cell_gap, 0.02);
    assert_eq!(ext.bus_line_width, 0.003);
    assert_eq!(ext.frame_border_width, 0.05);

    // Optical properties
    assert_eq!(ext.glass_reflectivity, 0.15);
    assert_eq!(ext.glass_roughness, 0.05);
    assert_eq!(ext.specular_intensity, 0.8);
}

/// Verifies SolarPanelMaterial can be constructed
#[test]
fn test_solar_panel_material_constructs() {
    let mat = SolarPanelMaterial {
        base: StandardMaterial::default(),
        extension: SolarPanelExtension::default(),
    };

    assert_eq!(mat.extension.cell_rows, 12.0);
    assert_eq!(mat.extension.cell_cols, 6.0);
}

/// Verifies material type path resolves correctly
#[test]
fn test_solar_panel_extension_shader_ref() {
    use bevy::pbr::MaterialExtension;
    let shader_ref = SolarPanelExtension::fragment_shader();
    // Should return a Handle reference, not a path string
    assert!(matches!(shader_ref, bevy::shader::ShaderRef::Handle(_)));
}
