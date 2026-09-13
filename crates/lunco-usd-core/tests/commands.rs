//! Tests for the shared USD command-contract helpers.

use lunco_usd_core::commands::is_usd_path;

#[test]
fn recognizes_supported_usd_extensions() {
    assert!(is_usd_path("/tmp/scene.usda"));
    assert!(is_usd_path("/tmp/scene.usd"));
    assert!(is_usd_path("scene.USD"));
    assert!(is_usd_path("foo/bar.usdc"));
    assert!(!is_usd_path("foo/bar.usdz"));
    assert!(!is_usd_path("/tmp/model.mo"));
    assert!(!is_usd_path("README.md"));
    assert!(!is_usd_path(""));
}
