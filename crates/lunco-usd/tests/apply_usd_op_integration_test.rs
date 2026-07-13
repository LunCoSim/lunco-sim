//! Integration tests for applying USD operations via the command bus.
//!
//! This verifies that the [`ApplyUsdOp`] command correctly mutates in-memory
//! USD stage documents, notifies listeners via [`DocumentChanged`], and propagates
//! updates through to the visual synchronization layer to update Bevy materials.

use bevy::prelude::*;
use lunco_usd_bevy::*;
use lunco_usd::{
    ApplyUsdOp, UsdCommandsPlugin, UsdDocumentRegistry, UsdOp, LayerId,
    ui::{SetActiveUsdViewport, UsdViewportPlugin, UsdViewportState},
};
use lunco_doc::DocumentOrigin;

/// Tests that triggering an [`ApplyUsdOp`] command modifies a shader attribute
/// (e.g. `diffuseColor` and `roughness`) in the underlying USD stage document,
/// and that the Bevy rendering system automatically updates the material.
#[test]
fn test_apply_usd_op_integration() {
    let mut app = App::new();

    // 1. Core Bevy plugins and asset registries. The viewport now mounts through
    // the `twin://` source, so register the lunco asset sources (which insert
    // `TwinRoots`) BEFORE `AssetPlugin` snapshots them.
    app.add_plugins(MinimalPlugins);
    lunco_assets::register_lunco_asset_sources(&mut app);
    app.add_plugins(AssetPlugin::default());

    app.init_asset::<UsdStageAsset>();
    app.init_asset::<Mesh>();
    app.init_asset::<StandardMaterial>();
    app.init_asset::<Image>();

    // 2. Add USD plugins
    app.add_plugins(UsdBevyPlugin);
    app.add_plugins(UsdCommandsPlugin);
    app.add_plugins(UsdViewportPlugin);

    // 3. Define initial USDA document containing a Material, Shader and a bound Cube Mesh
    let usda_content = r#"#usda 1.0
(
    defaultPrim = "World"
)

def Xform "World"
{
    def Material "MyMaterial"
    {
        token outputs:surface.connect = </World/MyMaterial/PbrShader.outputs:surface>

        def Shader "PbrShader"
        {
            uniform token info:id = "UsdPreviewSurface"
            color3f inputs:diffuseColor = (1.0, 0.5, 0.25)
            float inputs:roughness = 0.75
            float inputs:metallic = 0.25
            token outputs:surface
        }
    }

    def Cube "MeshWithMaterial" (
        apiSchemas = ["MaterialBindingAPI"]
    )
    {
        rel material:binding = </World/MyMaterial>
        double size = 2.0
    }
}
"#;

    // 4. Allocate USD document in the registry
    let doc_id = {
        let mut reg = app.world_mut().resource_mut::<UsdDocumentRegistry>();
        reg.allocate(
            usda_content.to_string(),
            DocumentOrigin::untitled("test_stage.usda".to_string()),
        )
    };
    println!("[TEST-DEBUG] Allocated doc_id: {:?}", doc_id);

    // 5. Trigger SetActiveUsdViewport command to bootstrap the preview stage
    // and install our newly allocated document into the active viewport
    app.world_mut().trigger(SetActiveUsdViewport { doc: doc_id });
    println!("[TEST-DEBUG] Triggered SetActiveUsdViewport");

    // Run updates to process the viewport installation and initial visual synchronization
    for i in 1..=5 {
        app.update();
        println!("[TEST-DEBUG] Tick {} complete.", i);
        
        // Print all entities and their components to trace spawning
        let mut q_debug = app.world_mut().query::<(Entity, Option<&Name>, Option<&UsdPrimPath>, Has<UsdVisualSynced>)>();
        for (ent, name, prim_path, synced) in q_debug.iter(app.world()) {
            println!(
                "  -> Entity: {:?}, Name: {:?}, PrimPath: {:?}, Synced: {}",
                ent,
                name.map(|n| n.as_str()),
                prim_path.map(|p| p.path.as_str()),
                synced
            );
        }
    }

    // Verify the child MeshWithMaterial entity was spawned and got its material
    let mut mesh_entity = None;
    let mut q = app.world_mut().query::<(Entity, &Name, &UsdPrimPath)>();
    for (ent, name, prim_path) in q.iter(app.world()) {
        if name.as_str().contains("MeshWithMaterial") {
            mesh_entity = Some(ent);
            assert_eq!(prim_path.path, "/World/MeshWithMaterial");
        }
    }
    let mesh_entity = mesh_entity.expect("MeshWithMaterial child entity should have been spawned");

    // Check appearance before applying any command.
    //
    // The prim carries a `PbrLook` (appearance INTENT), not a `MeshMaterial3d`: the
    // concrete material is bound by `LuncoRenderPlugin`, which a headless test does
    // not add. Asserting on the intent is strictly better here — it is what USD
    // authors, and it is what survives on a server with no GPU.
    // See docs/architecture/render-decoupling.md.
    let look = app.world().get::<lunco_render::PbrLook>(mesh_entity)
        .expect("Entity should have a PbrLook (the appearance authored by the USD material)");

    // Assert initial diffuse color (1.0, 0.5, 0.25) and roughness (0.75)
    assert!((look.base_color.red - 1.0).abs() < 1e-4);
    assert!((look.base_color.green - 0.5).abs() < 1e-4);
    assert!((look.base_color.blue - 0.25).abs() < 1e-4);
    assert!((look.perceptual_roughness - 0.75).abs() < 1e-4);

    // 6. Dispatch ApplyUsdOp commands to update the diffuse color and roughness
    let color_op = UsdOp::SetAttribute {
        edit_target: LayerId::root(),
        path: "/World/MyMaterial/PbrShader".to_string(),
        name: "inputs:diffuseColor".to_string(),
        type_name: "color3f".to_string(),
        value: "(0.0, 0.0, 1.0)".to_string(),
    };
    app.world_mut().trigger(ApplyUsdOp {
        doc: doc_id,
        op: color_op,
    });

    let roughness_op = UsdOp::SetAttribute {
        edit_target: LayerId::root(),
        path: "/World/MyMaterial/PbrShader".to_string(),
        name: "inputs:roughness".to_string(),
        type_name: "float".to_string(),
        value: "0.1".to_string(),
    };
    app.world_mut().trigger(ApplyUsdOp {
        doc: doc_id,
        op: roughness_op,
    });

    // 7. Run the Bevy app updates to process:
    // Update 1: command execution -> document mutation -> DocumentChanged -> viewport rebuild (clears UsdVisualSynced)
    app.update();
    // Update 2: sync_usd_visuals runs and recreates components with the new stage reader values
    app.update();

    // Verify updated values on the spawned child entity
    let mut updated_mesh_entity = None;
    let mut q2 = app.world_mut().query::<(Entity, &Name, &UsdPrimPath)>();
    for (ent, name, _prim_path) in q2.iter(app.world()) {
        if name.as_str().contains("MeshWithMaterial") {
            updated_mesh_entity = Some(ent);
        }
    }
    let updated_mesh_entity = updated_mesh_entity.expect("MeshWithMaterial should exist after reload");

    let look2 = app.world().get::<lunco_render::PbrLook>(updated_mesh_entity)
        .expect("Entity should have a PbrLook after reload");

    // Assert the appearance updated to Blue (0.0, 0.0, 1.0) and roughness 0.1
    assert!((look2.base_color.red - 0.0).abs() < 1e-4);
    assert!((look2.base_color.green - 0.0).abs() < 1e-4);
    assert!((look2.base_color.blue - 1.0).abs() < 1e-4);
    assert!((look2.perceptual_roughness - 0.1).abs() < 1e-4);

    // 7b. Exercise the coarse full-reload path explicitly: a whole-source
    // `ReplaceSource` (the genuine whole-layer revert, and still the inverse for a
    // newly-authored attribute). This must rebuild the live stage from the
    // (reverted) composed source and re-instantiate, so the material reverts in the
    // live world too — the regression guard for the full-reload attribute path.
    // (Overwrites of an *existing* attribute now invert to a typed `SetAttribute`
    // instead — see `document::set_attribute_overwrite_inverts_to_typed_op`.)
    app.world_mut().trigger(ApplyUsdOp {
        doc: doc_id,
        op: UsdOp::ReplaceSource {
            edit_target: LayerId::root(),
            text: usda_content.to_string(),
        },
    });
    // Rebuild + re-instantiate takes a couple of frames (doc mutation → stage
    // rebuild → refresh_scene_visuals → observer re-instantiates the subtree).
    for _ in 0..6 {
        app.update();
    }

    let mut reverted_entity = None;
    let mut q3 = app.world_mut().query::<(Entity, &Name, &UsdPrimPath)>();
    for (ent, name, _) in q3.iter(app.world()) {
        if name.as_str().contains("MeshWithMaterial") {
            reverted_entity = Some(ent);
        }
    }
    let reverted_entity = reverted_entity.expect("MeshWithMaterial must survive the ReplaceSource rebuild");
    let look3 = app.world().get::<lunco_render::PbrLook>(reverted_entity)
        .expect("Entity should have a PbrLook after the full-reload rebuild");
    // Back to the original diffuse (1.0, 0.5, 0.25) + roughness 0.75.
    assert!((look3.base_color.red - 1.0).abs() < 1e-4,
        "base_color red must revert after full-reload, got {:?}", look3.base_color);
    assert!((look3.base_color.green - 0.5).abs() < 1e-4);
    assert!((look3.base_color.blue - 0.25).abs() < 1e-4);
    assert!((look3.perceptual_roughness - 0.75).abs() < 1e-4,
        "roughness must revert to 0.75 after full-reload, got {}", look3.perceptual_roughness);

    // 8. Confirm viewport state has been updated
    let state = app.world().resource::<UsdViewportState>();
    assert_eq!(state.active_doc(), Some(doc_id));
}
