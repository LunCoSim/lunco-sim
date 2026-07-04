/// Check whether ramp/ground prims in the sandbox scene have the right
/// collider attributes after USD composition.

use lunco_usd_bevy::{StageView, UsdRead};
use openusd::sdf::Path as SdfPath;
use std::path::Path as FilePath;

#[test]
fn check_sandbox_colliders() {
    let p = FilePath::new(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap().parent().unwrap()
        .join("assets/scenes/sandbox/sandbox_scene.usda");
    let stage = lunco_usd_bevy::compose_file_to_stage(&p).expect("compose sandbox_scene");
    let view = StageView::new(&stage);

    println!("\n===== ALL PRIMS WITH PhysicsCollisionAPI =====");
    for path in view.prim_paths() {
        if !view.has_api_schema(&path, "PhysicsCollisionAPI") { continue; }

        let ty = view.prim_type_name(&path).unwrap_or_default();
        let vis: Option<String> = view.value_str(&path, "visibility");
        let col_en: Option<bool> = view.value(&path, "physics:collisionEnabled");
        let rigid: Option<bool> = view.value(&path, "physics:rigidBodyEnabled");
        let scale: Option<[f64; 3]> = view.value(&path, "xformOp:scale");
        let size: Option<f64> = view.value(&path, "size");
        let parent = path.parent().map(|p| p.as_str().to_string());

        println!("  {} type={} vis={:?} coll={:?} rigid={:?} size={:?} scale={:?} parent={:?}",
            path.as_str(), ty, vis, col_en, rigid, size, scale, parent);
    }

    // Also check: does the ground have PhysxTerrainAPI?
    let ground = SdfPath::new("/SandboxScene/Ground").unwrap();
    println!("\n===== GROUND =====");
    dump_prim(&view, &ground);

    let ramp = SdfPath::new("/SandboxScene/Ramp").unwrap();
    println!("\n===== RAMP =====");
    dump_prim(&view, &ramp);

    let ramp1 = SdfPath::new("/SandboxScene/Ramp1").unwrap();
    println!("\n===== RAMP1 =====");
    dump_prim(&view, &ramp1);
}

fn dump_prim(view: &StageView<'_>, path: &SdfPath) {
    let ty = view.prim_type_name(path).unwrap_or_default();
    let vis: Option<String> = view.value_str(path, "visibility");
    let col_en: Option<bool> = view.value(path, "physics:collisionEnabled");
    let rigid: Option<bool> = view.value(path, "physics:rigidBodyEnabled");
    let active = view.is_active(path);
    let scale: Option<[f64; 3]> = view.value(path, "xformOp:scale");
    let size: Option<f64> = view.value(path, "size");
    // TODO(usd-read-migration): switch to the generic UsdRead surface (`children`)
    // instead of the legacy `prim_children`, matching production (doc 21).
    let children = view.prim_children(path);

    // Check has_api_schema the same way the avian code does
    let has_collision = view.has_api_schema(path, "PhysicsCollisionAPI");
    let has_terrain = view.has_api_schema(path, "PhysxTerrainAPI");

    println!("  type={}", ty);
    println!("  active={}", active);
    println!("  visibility={:?}", vis);
    println!("  collisionEnabled={:?}", col_en);
    println!("  rigidBodyEnabled={:?}", rigid);
    println!("  size={:?}", size);
    println!("  scale={:?}", scale);
    println!("  children={:?}", children.iter().map(|c| c.as_str().to_string()).collect::<Vec<_>>());
    println!("  has_api_schema(PhysicsCollisionAPI) = {}", has_collision);
    println!("  has_api_schema(PhysxTerrainAPI) = {}", has_terrain);
}
