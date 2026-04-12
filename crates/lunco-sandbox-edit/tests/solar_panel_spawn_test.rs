//! Test that verifies solar panel USD file has correct schema.

/// Verifies the solar_panel.usda file has the correct PhysicsRigidBodyAPI schema
#[test]
fn test_solar_panel_usda_has_rigid_body_api() {
    // Path relative to workspace root, we need to find it from crate dir
    let usda_path = std::path::Path::new("../../assets/components/power/solar_panel.usda");
    let content = std::fs::read_to_string(usda_path)
        .unwrap_or_else(|e| panic!("solar_panel.usda should exist at {:?}: {}", usda_path, e));
    assert!(
        content.contains("PhysicsRigidBodyAPI"),
        "Solar panel root Xform must have PhysicsRigidBodyAPI in apiSchemas"
    );
    assert!(
        content.contains("PhysicsCollisionAPI"),
        "PanelFrame and PanelSurface must have PhysicsCollisionAPI"
    );
    // Children should NOT have physics:rigidBodyEnabled
    assert!(
        !content.contains("physics:rigidBodyEnabled = true"),
        "Children should NOT have physics:rigidBodyEnabled = true (they are colliders under a compound body)"
    );
}

/// Verifies the solar_panel.usda has the correct material type primvar
#[test]
fn test_solar_panel_usda_has_material_type() {
    let usda_path = std::path::Path::new("../../assets/components/power/solar_panel.usda");
    let content = std::fs::read_to_string(usda_path)
        .unwrap_or_else(|e| panic!("solar_panel.usda should exist at {:?}: {}", usda_path, e));
    assert!(
        content.contains("primvars:materialType = \"solar_panel\""),
        "PanelSurface must have primvars:materialType = solar_panel"
    );
}

/// Verifies the catalog entry for solar_panel exists with correct config
#[test]
fn test_solar_panel_catalog_entry() {
    let catalog = lunco_sandbox_edit::catalog::SpawnCatalog::default();
    let entry = catalog.get("solar_panel").expect("solar_panel should exist");
    assert_eq!(entry.display_name, "Solar Panel");
    assert_eq!(entry.category, lunco_sandbox_edit::catalog::SpawnCategory::Component);
}
