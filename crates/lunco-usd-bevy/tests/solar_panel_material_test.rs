//! Unit tests for lunco-usd-bevy crate.

/// Verifies the UsdBevyPlugin exists and is constructible
#[test]
fn test_usd_bevy_plugin_constructs() {
    let _plugin = lunco_usd_bevy::UsdBevyPlugin;
}

/// Verifies UsdPrimPath is constructible
#[test]
fn test_usd_prim_path_constructs() {
    use bevy::prelude::*;
    use lunco_usd_bevy::UsdPrimPath;

    let path = UsdPrimPath {
        stage_handle: Handle::default(),
        path: "/Test".to_string(),
    };

    assert_eq!(path.path, "/Test");
}
