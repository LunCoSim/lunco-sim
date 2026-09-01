//! Integration test for the **sink-driven structural projection** path (#1/#2):
//! an `AddPrim` op on a doc-backed viewport scene is authored onto the live
//! `CanonicalStage`, whose openusd change sink fires and `project_stage_changes`
//! spawns the matching ECS entity — with **no whole-scene asset reload**. This
//! is the end-to-end regression for the incremental projection path: the twin
//! projection systems (`sync_twin_overlays` → typed live authoring →
//! `project_stage_changes`) now drive incremental structural edits.

use bevy::prelude::*;
use lunco_doc_bevy::DocumentRegistry;
use lunco_usd::document::UsdDocument;
use lunco_usd::{
    ui::{CloseUsdPreview, FocusUsdPreview, OpenUsdPreview, UsdPreviewId, UsdViewportPlugin},
    ApplyUsdOp, LayerId, UsdCommandsPlugin, UsdOp,
};
use lunco_usd_bevy::*;

mod support;

/// True when an entity projecting `path` (in any live scene) exists.
fn has_prim_entity(app: &mut App, path: &str) -> bool {
    let mut q = app.world_mut().query::<&UsdPrimPath>();
    q.iter(app.world()).any(|p| p.path == path)
}

/// How many live entities project a prim strictly under `prefix` (e.g. children
/// pulled in by a reference arc).
fn prims_under(app: &mut App, prefix: &str) -> usize {
    let mut q = app.world_mut().query::<&UsdPrimPath>();
    q.iter(app.world())
        .filter(|p| p.path.starts_with(prefix))
        .count()
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
    app.init_resource::<lunco_core::CommandResults>()
        .init_resource::<lunco_core::ActiveCommandId>();
    app.add_plugins(UsdViewportPlugin);
    app
}

#[test]
fn add_prim_projects_live_via_sink_no_reload() {
    let mut app = boot_app();

    // A minimal scene: one Xform the spawn will hang under.
    let usda = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\"\n{\n}\n";
    let doc = {
        let mut reg = app
            .world_mut()
            .resource_mut::<DocumentRegistry<UsdDocument>>();
        reg.allocate(
            usda.to_string(),
            lunco_doc::PathlessOrigin::untitled("live_spawn.usda"),
        )
    };

    // Open its explicit preview lease → doc-backed Twin scene → async mount →
    // CanonicalStage built. Tick until the scene root projects.
    app.world_mut().trigger(OpenUsdPreview {
        preview: UsdPreviewId(1),
        doc,
        edit_target: LayerId::root(),
    });
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
    // rides `sync_twin_overlays` → typed live authoring (Plain) → the live
    // stage's sink → `project_stage_changes`, spawning the entity in place.
    app.world_mut().trigger(ApplyUsdOp {
        doc,
        parent_gen: None,
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
        let mut reg = app
            .world_mut()
            .resource_mut::<DocumentRegistry<UsdDocument>>();
        reg.allocate(
            usda.to_string(),
            lunco_doc::PathlessOrigin::untitled("ref_spawn.usda"),
        )
    };

    app.world_mut().trigger(OpenUsdPreview {
        preview: UsdPreviewId(1),
        doc,
        edit_target: LayerId::root(),
    });
    for _ in 0..40 {
        app.update();
        if has_prim_entity(&mut app, "/World") {
            break;
        }
    }
    assert!(
        has_prim_entity(&mut app, "/World"),
        "scene root must project first"
    );

    // Spawn a rover by reference through the location-independent `lunco://`
    // source (→ `<workspace>/assets/vessels/rovers/skid_rover.usda`), so it
    // resolves regardless of the viewport twin or the cargo-test manifest dir.
    app.world_mut().trigger(ApplyUsdOp {
        doc,
        parent_gen: None,
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

/// Two assembly documents may author the same prim paths. Each explicit
/// preview lease retains its own stage root, camera, and render layer; focus
/// changes only the dock surface and does not destroy the other document.
#[test]
fn simultaneous_assembly_previews_keep_identical_paths_isolated() {
    let mut app = boot_app();
    let first_source = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\"\n{\n    def Cube \"First\" {}\n}\n";
    let second_source = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\"\n{\n    def Cube \"Second\" {}\n}\n";
    let (first_doc, second_doc) = {
        let mut registry = app
            .world_mut()
            .resource_mut::<DocumentRegistry<UsdDocument>>();
        (
            registry.allocate(
                first_source.to_string(),
                lunco_doc::PathlessOrigin::untitled("first-assembly.usda"),
            ),
            registry.allocate(
                second_source.to_string(),
                lunco_doc::PathlessOrigin::untitled("second-assembly.usda"),
            ),
        )
    };

    app.world_mut().trigger(OpenUsdPreview {
        preview: UsdPreviewId(1),
        doc: first_doc,
        edit_target: LayerId::root(),
    });
    support::settle_visual_projection(&mut app);
    let first_stage = app
        .world()
        .resource::<lunco_usd::ui::UsdViewportState>()
        .session(UsdPreviewId(1))
        .expect("first assembly has an open preview")
        .stage_handle()
        .id();
    let first_root = app
        .world()
        .resource::<lunco_usd::ui::UsdViewportState>()
        .session(UsdPreviewId(1))
        .unwrap()
        .scene_root();
    assert!(has_prim_entity(&mut app, "/World/First"));

    app.world_mut().trigger(OpenUsdPreview {
        preview: UsdPreviewId(2),
        doc: second_doc,
        edit_target: LayerId::runtime(),
    });
    support::settle_visual_projection(&mut app);
    let state = app.world().resource::<lunco_usd::ui::UsdViewportState>();
    let second_session = state
        .session(UsdPreviewId(2))
        .expect("second assembly has an open preview");
    let second_handle = second_session.stage_handle().id();
    let second_root = second_session.scene_root();
    assert_ne!(first_stage, second_handle);
    assert_ne!(first_root, second_root);
    assert_ne!(
        state.session(UsdPreviewId(1)).unwrap().render_layer(),
        second_session.render_layer()
    );
    assert_eq!(state.focused_doc(), Some(second_doc));

    let mut prims = app.world_mut().query::<&UsdPrimPath>();
    let projected: Vec<_> = prims
        .iter(app.world())
        .filter(|prim| prim.path == "/World/First" || prim.path == "/World/Second")
        .map(|prim| (prim.stage_handle.id(), prim.path.clone()))
        .collect();
    assert!(projected.contains(&(first_stage, "/World/First".to_string())));
    assert!(projected.contains(&(second_handle, "/World/Second".to_string())));

    // Focus is a presentation change only. The first lease keeps its own
    // projected subtree while the second remains open.
    app.world_mut().trigger(FocusUsdPreview {
        preview: UsdPreviewId(1),
    });
    app.update();
    let state = app.world().resource::<lunco_usd::ui::UsdViewportState>();
    assert_eq!(state.focused_doc(), Some(first_doc));
    assert_eq!(state.session_count(), 2);
    assert!(state.session(UsdPreviewId(2)).is_some());

    // Closing one lease removes only its presentation entities and releases
    // no shared document authority still owned by the other lease.
    app.world_mut().trigger(CloseUsdPreview {
        preview: UsdPreviewId(2),
    });
    app.update();
    let state = app.world().resource::<lunco_usd::ui::UsdViewportState>();
    assert_eq!(state.session_count(), 1);
    assert!(state.session(UsdPreviewId(1)).is_some());
    assert!(state.session(UsdPreviewId(2)).is_none());
    assert!(app.world().get_entity(second_root).is_err());
    assert!(app
        .world()
        .resource::<lunco_usd::twin_projection::DocBackedTwinScenes>()
        .coords_of(second_doc)
        .is_none());
    assert!(app
        .world()
        .resource::<lunco_usd::twin_projection::DocBackedTwinScenes>()
        .coords_of(first_doc)
        .is_some());
}
