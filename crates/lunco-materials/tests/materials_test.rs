//! Unit tests for lunco-materials crate.

use lunco_materials::{SolarPanelExtension, BlueprintExtension};

/// Verifies SolarPanelExtension has sensible default values
#[test]
fn test_solar_panel_extension_defaults() {
    let ext = SolarPanelExtension::default();
    assert_eq!(ext.panel_half_width, 3.0);
    assert_eq!(ext.cell_rows, 12.0);
    assert!(ext.cell_color.blue > ext.cell_color.red);
}

/// Verifies BlueprintExtension has sensible default values
#[test]
fn test_blueprint_extension_defaults() {
    let ext = BlueprintExtension::default();
    assert_eq!(ext.major_grid_spacing, 1.0);
    assert!(ext.surface_color.red < 0.5);
}
