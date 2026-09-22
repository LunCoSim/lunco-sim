use std::path::PathBuf;

use lunco_assets_core::{transitive_file_closure, transitive_file_closure_with};
use lunco_assets_path::normalize;
use lunco_usd_compose::{compose_file_to_stage_with_roots, is_usd_layer, layer_dependency_arcs};

#[test]
fn follows_composition_and_asset_attribute_dependencies() {
    let dir = tempfile::tempdir().unwrap();
    let scene = dir.path().join("scene.usda");
    let rover = dir.path().join("rover.usda");
    let model = dir.path().join("Drive.mo");
    lunco_storage::write_file_sync(
        &scene,
        b"#usda 1.0\ndef Xform \"R\" (prepend references = @rover.usda@) {}\n",
    )
    .unwrap();
    lunco_storage::write_file_sync(
        &rover,
        b"#usda 1.0\ndef Xform \"P\" { asset info:sourceAsset = @Drive.mo@ }\n",
    )
    .unwrap();
    lunco_storage::write_file_sync(&model, b"model Drive end Drive;\n").unwrap();

    let closure = transitive_file_closure(&[scene], is_usd_layer, layer_dependency_arcs);
    assert!(closure.contains(&normalize(&rover)), "{closure:?}");
    assert!(closure.contains(&normalize(&model)), "{closure:?}");
}

#[test]
fn delegates_schemed_reference_resolution_to_the_asset_caller() {
    let dir = tempfile::tempdir().unwrap();
    let assets = dir.path().join("assets");
    let scene = dir.path().join("scene.usda");
    let rover = assets.join("vessels/rover.usda");
    lunco_storage::ensure_directory_sync(rover.parent().unwrap()).unwrap();
    lunco_storage::write_file_sync(
        &scene,
        b"#usda 1.0\ndef Xform \"R\" (prepend references = @lunco://vessels/rover.usda@) {}\n",
    )
    .unwrap();
    lunco_storage::write_file_sync(&rover, b"#usda 1.0\n").unwrap();

    let closure = transitive_file_closure_with(
        &[PathBuf::from(&scene)],
        |arc| lunco_assets_core::parse_lunco_uri(arc).map(|relative| assets.join(relative)),
        is_usd_layer,
        layer_dependency_arcs,
    );
    assert!(closure.contains(&normalize(&rover)), "{closure:?}");
}

#[test]
fn composes_available_siblings_when_one_layer_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let scene = dir.path().join("scene.usda");
    let available = dir.path().join("available.usda");
    lunco_storage::write_file_sync(
        &scene,
        br#"#usda 1.0
(
    subLayers = [
        @missing.usda@,
        @available.usda@
    ]
)
def Xform "Root" {}
"#,
    )
    .unwrap();
    lunco_storage::write_file_sync(
        &available,
        br#"#usda 1.0
def Xform "Available" {}
"#,
    )
    .unwrap();

    compose_file_to_stage_with_roots(&scene, None, None)
        .expect("a missing sibling layer must not discard available USD content");
}
