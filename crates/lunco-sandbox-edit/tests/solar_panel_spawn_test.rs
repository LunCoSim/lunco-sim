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

/// Verifies the solar_panel.usda drives the general ShaderMaterial.
#[test]
fn test_solar_panel_usda_selects_its_shader() {
    let usda_path = std::path::Path::new("../../assets/components/power/solar_panel.usda");
    let content = std::fs::read_to_string(usda_path)
        .unwrap_or_else(|e| panic!("solar_panel.usda should exist at {:?}: {}", usda_path, e));
    // `asset`, not `string` — naming the shader IS selecting it.
    assert!(
        content.contains("uniform asset lunco:material:shader = @shaders/solar_panel.wgsl@"),
        "PanelSurface must point lunco:material:shader at shaders/solar_panel.wgsl"
    );
    assert!(
        content.contains("LunCoMaterialAPI"),
        "a prim carrying lunco:material:* must apply LunCoMaterialAPI"
    );
}

/// Verifies the solar_panel is *discovered* into the catalog with the right
/// display name and folder-derived category.
///
/// The catalog is fully data-driven now: `SpawnCatalog::default()` is empty and
/// every spawnable is found at runtime by scanning the project USD, with its
/// category derived from the parent folder (no hardcoded `SpawnCategory` enum).
/// So `components/power/solar_panel.usda` yields display "Solar Panel" + category
/// "Power" (the immediate folder, title-cased), not a Rust-side taxonomy.
///
/// Drives the *blocking* scan — the app's is async (a web read is an HTTP fetch),
/// but both run the same enumerate → read → `parse_spawn_meta` → `entry_for`
/// path, so this still exercises the real pipeline end to end.
#[test]
fn test_solar_panel_catalog_entry() {
    use lunco_assets::twin_source::TwinRoots;

    // cwd under `cargo test` is the crate dir, so reach the workspace `assets/`
    // via the compile-time manifest dir (same hop the USD-file tests above use).
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let roots = TwinRoots::default();
    roots.register("assets", assets);

    // The workspace `assets/` is registered above as a Twin ROOT, so the Twin walk
    // discovers it with correct absolute paths regardless of cwd (`cargo test` runs
    // in the crate dir, not the workspace root). The engine-library manifest — the
    // *other* half of discovery — is therefore empty here, but must still be marked
    // ready: unready means "not known yet", and the scan would rightly wait.
    let mut manifest = lunco_assets::discovery::AssetManifest::default();
    manifest.set(Vec::new());

    let mut catalog = lunco_sandbox_edit::catalog::SpawnCatalog::default();
    lunco_sandbox_edit::catalog::scan_usd_into_catalog_blocking(&manifest, &roots, &mut catalog);

    let entry = catalog
        .get("solar_panel")
        .expect("solar_panel.usda should be discovered under assets/");
    assert_eq!(entry.display_name, "Solar Panel");
    assert_eq!(entry.category, "Power");
}
