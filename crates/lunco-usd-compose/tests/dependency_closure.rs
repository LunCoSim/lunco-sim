use std::path::PathBuf;

use lunco_assets::{asset_path::normalize, transitive_file_closure, transitive_file_closure_with};
use lunco_usd_compose::{is_usd_layer, layer_dependency_arcs};

#[test]
fn follows_composition_and_asset_attribute_dependencies() {
    let dir = tempfile::tempdir().unwrap();
    let scene = dir.path().join("scene.usda");
    let rover = dir.path().join("rover.usda");
    let model = dir.path().join("Drive.mo");
    std::fs::write(
        &scene,
        "#usda 1.0\ndef Xform \"R\" (prepend references = @rover.usda@) {}\n",
    )
    .unwrap();
    std::fs::write(
        &rover,
        "#usda 1.0\ndef Xform \"P\" { asset info:sourceAsset = @Drive.mo@ }\n",
    )
    .unwrap();
    std::fs::write(&model, "model Drive end Drive;\n").unwrap();

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
    std::fs::create_dir_all(rover.parent().unwrap()).unwrap();
    std::fs::write(
        &scene,
        "#usda 1.0\ndef Xform \"R\" (prepend references = @lunco://vessels/rover.usda@) {}\n",
    )
    .unwrap();
    std::fs::write(&rover, "#usda 1.0\n").unwrap();

    let closure = transitive_file_closure_with(
        &[PathBuf::from(&scene)],
        |arc| lunco_assets::parse_lunco_uri(arc).map(|relative| assets.join(relative)),
        is_usd_layer,
        layer_dependency_arcs,
    );
    assert!(closure.contains(&normalize(&rover)), "{closure:?}");
}
