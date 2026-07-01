/// Check whether ramp/ground prims in the sandbox scene have the right
/// collider attributes after USD composition.

use lunco_usd_bevy::compose_file;
use lunco_usd_bevy::usd_data::UsdDataExt;
use openusd::sdf::{AbstractData, Path as SdfPath};
use std::path::Path as FilePath;

#[test]
fn check_sandbox_colliders() {
    let p = FilePath::new(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap().parent().unwrap()
        .join("assets/scenes/sandbox/sandbox_scene.usda");
    let composed = compose_file(&p).expect("compose sandbox_scene");

    println!("\n===== ALL PRIMS WITH PhysicsCollisionAPI =====");
    for (path, spec) in composed.iter() {
        if spec.ty != openusd::sdf::SpecType::Prim { continue; }
        let apis = composed.field(path, "apiSchemas");
        let has_collision = match apis {
            Some(openusd::sdf::Value::TokenListOp(op)) => {
                op.explicit_items.iter().chain(op.prepended_items.iter())
                    .chain(op.appended_items.iter()).chain(op.added_items.iter())
                    .any(|s| s.as_str().contains("PhysicsCollisionAPI"))
            }
            Some(openusd::sdf::Value::TokenVec(v)) => v.iter().any(|s| s.as_str().contains("PhysicsCollisionAPI")),
            Some(openusd::sdf::Value::Token(s)) => s.as_str().contains("PhysicsCollisionAPI"),
            _ => false,
        };
        if !has_collision { continue; }

        let ty = composed.prim_type_name(path).unwrap_or_default();
        let vis: Option<String> = composed.prim_attribute_value(path, "visibility");
        let col_en: Option<bool> = composed.prim_attribute_value(path, "physics:collisionEnabled");
        let rigid: Option<bool> = composed.prim_attribute_value(path, "physics:rigidBodyEnabled");
        let scale: Option<[f64; 3]> = composed.prim_attribute_value(path, "xformOp:scale");
        let size: Option<f64> = composed.prim_attribute_value(path, "size");
        let parent = path.parent().map(|p| p.as_str().to_string());

        println!("  {} type={} vis={:?} coll={:?} rigid={:?} size={:?} scale={:?} parent={:?}",
            path.as_str(), ty, vis, col_en, rigid, size, scale, parent);
    }

    // Also check: does the ground have PhysxTerrainAPI?
    let ground = SdfPath::new("/SandboxScene/Ground").unwrap();
    println!("\n===== GROUND =====");
    dump_prim(&composed, &ground);

    let ramp = SdfPath::new("/SandboxScene/Ramp").unwrap();
    println!("\n===== RAMP =====");
    dump_prim(&composed, &ramp);

    let ramp1 = SdfPath::new("/SandboxScene/Ramp1").unwrap();
    println!("\n===== RAMP1 =====");
    dump_prim(&composed, &ramp1);
}

fn dump_prim(data: &openusd::sdf::Data, path: &SdfPath) {
    use openusd::sdf::Value;
    let ty = data.prim_type_name(path).unwrap_or_default();
    let apis = data.field(path, "apiSchemas");
    let vis: Option<String> = data.prim_attribute_value(path, "visibility");
    let col_en: Option<bool> = data.prim_attribute_value(path, "physics:collisionEnabled");
    let rigid: Option<bool> = data.prim_attribute_value(path, "physics:rigidBodyEnabled");
    let active = data.prim_is_active(path);
    let scale: Option<[f64; 3]> = data.prim_attribute_value(path, "xformOp:scale");
    let size: Option<f64> = data.prim_attribute_value(path, "size");
    let children = data.prim_children(path);

    println!("  type={}", ty);
    println!("  active={}", active);
    println!("  apiSchemas={:?}", apis);  // raw Value
    println!("  visibility={:?}", vis);
    println!("  collisionEnabled={:?}", col_en);
    println!("  rigidBodyEnabled={:?}", rigid);
    println!("  size={:?}", size);
    println!("  scale={:?}", scale);
    println!("  children={:?}", children.iter().map(|c| c.as_str().to_string()).collect::<Vec<_>>());

    // Check has_api_schema the same way the avian code does
    let has_collision = match apis {
        Some(Value::TokenListOp(op)) => {
            op.explicit_items.iter().chain(op.prepended_items.iter())
                .chain(op.appended_items.iter()).chain(op.added_items.iter())
                .any(|s| s.as_str() == "PhysicsCollisionAPI")
        }
        Some(Value::TokenVec(v)) => v.iter().any(|s| s.as_str() == "PhysicsCollisionAPI"),
        Some(Value::Token(s)) => s.as_str() == "PhysicsCollisionAPI",
        _ => false,
    };
    let has_terrain = match apis {
        Some(Value::TokenListOp(op)) => {
            op.explicit_items.iter().chain(op.prepended_items.iter())
                .chain(op.appended_items.iter()).chain(op.added_items.iter())
                .any(|s| s.as_str() == "PhysxTerrainAPI")
        }
        Some(Value::TokenVec(v)) => v.iter().any(|s| s.as_str() == "PhysxTerrainAPI"),
        Some(Value::Token(s)) => s.as_str() == "PhysxTerrainAPI",
        _ => false,
    };
    println!("  has_api_schema(PhysicsCollisionAPI) = {}", has_collision);
    println!("  has_api_schema(PhysxTerrainAPI) = {}", has_terrain);
}
