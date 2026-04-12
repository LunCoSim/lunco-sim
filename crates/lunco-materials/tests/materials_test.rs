//! Unit tests for lunco-materials crate.

use lunco_materials::{SolarPanelExtension, BlueprintExtension};

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
