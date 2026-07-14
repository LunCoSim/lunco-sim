//! Integration test for the **sink-driven structural projection** path (#1/#2):
//! an `AddPrim` op on a doc-backed viewport scene is authored onto the live
//! `CanonicalStage`, whose openusd change sink fires and `project_stage_changes`
//! spawns the matching ECS entity — with **no whole-scene asset reload**. This
//! is the end-to-end regression for the retired reload machinery: the twin
//! projection systems (`sync_twin_overlays` → `author_structural_edit` →
//! `project_stage_changes`) now drive incremental structural edits.

use bevy::prelude::*;
use lunco_doc::DocumentOrigin;
use lunco_usd::{
    ui::{SetActiveUsdViewport, UsdViewportPlugin},
    ApplyUsdOp, LayerId, UsdCommandsPlugin, UsdDocumentRegistry, UsdOp,
};
use lunco_usd_bevy::*;

/// True when an entity projecting `path` (in any live scene) exists.
fn has_prim_entity(app: &mut App, path: &str) -> bool {
    let mut q = app.world_mut().query::<&UsdPrimPath>();
    q.iter(app.world()).any(|p| p.path == path)
}

/// How many live entities project a prim strictly under `prefix` (e.g. children
/// pulled in by a reference arc).
fn prims_under(app: &mut App, prefix: &str) -> usize {
    let mut q = app.world_mut().query::<&UsdPrimPath>();
    q.iter(app.world()).filter(|p| p.path.starts_with(prefix)).count()
}

/// Boot a doc-backed viewport app with the twin asset source wired.
fn boot_app() -> App {
    // Asset sources root at `current_dir()/assets`; under `cargo test` that's the
    // crate dir, so anchor it at the workspace root (deterministic — every test
    // thread sets the same path, no race) so `/vessels/...` references resolve to
    // the shipped `networking/assets/vessels/...`.
    let ws_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let _ = std::env::set_current_dir(&ws_root);

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    // Twin asset source + `TwinRoots` — must be registered BEFORE `AssetPlugin`
    // (Bevy snapshots asset sources at its build). The doc-backed viewport mounts
    // through the `twin://` source, so the projection path needs it.
    lunco_assets::register_lunco_asset_sources(&mut app);
    app.add_plugins(AssetPlugin::default());
    app.init_asset::<UsdStageAsset>();
    app.init_asset::<Mesh>();
    app.init_asset::<Image>();
    app.add_plugins(UsdBevyPlugin);
    app.add_plugins(UsdCommandsPlugin);
    app.add_plugins(UsdViewportPlugin);
    app
}

#[test]
fn add_prim_projects_live_via_sink_no_reload() {
    let mut app = boot_app();

    // A minimal scene: one Xform the spawn will hang under.
    let usda = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\"\n{\n}\n";
    let doc = {
        let mut reg = app.world_mut().resource_mut::<UsdDocumentRegistry>();
        reg.allocate(usda.to_string(), DocumentOrigin::untitled("live_spawn.usda"))
    };

    // Install it as the active viewport → doc-backed twin scene → async mount →
    // CanonicalStage built. Tick until the scene root projects.
    app.world_mut().trigger(SetActiveUsdViewport { doc });
    for _ in 0..40 {
        app.update();
        if has_prim_entity(&mut app, "/World") {
            break;
        }
    }
    assert!(
        has_prim_entity(&mut app, "/World"),
        "the scene root must project onto the live world before we spawn into it"
    );
    assert!(
        !has_prim_entity(&mut app, "/World/Box"),
        "the spawn target must not exist yet"
    );

    // Author a plain (reference-less) child prim into the runtime layer. This
    // rides `sync_twin_overlays` → `author_structural_edit` (Plain) → the live
    // stage's sink → `project_stage_changes`, spawning the entity in place.
    app.world_mut().trigger(ApplyUsdOp {
        doc,
        op: UsdOp::AddPrim {
            edit_target: LayerId::runtime(),
            parent_path: "/World".into(),
            name: "Box".into(),
            type_name: Some("Cube".into()),
            reference: None,
        },
    });

    // A few ticks for: command → doc mutation → sync_twin_overlays authors onto
    // the stage → sink drains → project spawns → observer builds the subtree.
    for _ in 0..10 {
        app.update();
        if has_prim_entity(&mut app, "/World/Box") {
            break;
        }
    }
    assert!(
        has_prim_entity(&mut app, "/World/Box"),
        "the authored prim must project into a live entity through the sink bridge (no reload)"
    );
}

/// The keystone of #1 end-to-end: a **referenced** spawn (a prim that references
/// a real on-disk asset not yet loaded into the scene) is fetched once through
/// `drain_ref_spawns`, its closure injected into the live resolver, and the
/// reference authored onto the live stage — so PCP composes the referenced
/// subtree and `project_stage_changes` instantiates it, with no whole-scene
/// reload. Uses the shipped `skid_rover.usda` (leading-slash asset-root ref, so
/// it resolves through the default asset source regardless of the viewport twin).
#[test]
fn referenced_spawn_projects_live_via_fetch_inject_author() {
    let mut app = boot_app();

    let usda = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\"\n{\n}\n";
    let doc = {
        let mut reg = app.world_mut().resource_mut::<UsdDocumentRegistry>();
        reg.allocate(usda.to_string(), DocumentOrigin::untitled("ref_spawn.usda"))
    };

    app.world_mut().trigger(SetActiveUsdViewport { doc });
    for _ in 0..40 {
        app.update();
        if has_prim_entity(&mut app, "/World") {
            break;
        }
    }
    assert!(has_prim_entity(&mut app, "/World"), "scene root must project first");

    // Spawn a rover by reference through the location-independent `lunco://`
    // source (→ `<workspace>/assets/vessels/rovers/skid_rover.usda`), so it
    // resolves regardless of the viewport twin or the cargo-test manifest dir.
    app.world_mut().trigger(ApplyUsdOp {
        doc,
        op: UsdOp::AddPrim {
            edit_target: LayerId::runtime(),
            parent_path: "/World".into(),
            name: "rover_1".into(),
            type_name: Some("Xform".into()),
            reference: Some("lunco://vessels/rovers/skid_rover.usda".into()),
        },
    });

    // Generous budget: the op mutates the doc, `sync_twin_overlays` queues the
    // referenced spawn, the asset loader fetches the rover's `.usda` closure
    // (several async frames — the shared task pool may be busy when other tests
    // run concurrently, so budget high), `drain_ref_spawns` injects + authors,
    // then the sink projects — spawn root plus the composed subtree.
    for _ in 0..400 {
        app.update();
        if prims_under(&mut app, "/World/rover_1/") > 0 {
            break;
        }
    }
    assert!(
        has_prim_entity(&mut app, "/World/rover_1"),
        "the referenced spawn root must project"
    );
    assert!(
        prims_under(&mut app, "/World/rover_1/") > 0,
        "the referenced rover's composed subtree must project under the spawn \
         (fetch → inject → author → sink), proving the reference composed live"
    );
}
