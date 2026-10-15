//! Generic integration seams for sink-driven structural projection and explicit
//! document preview leases. A committed stage edit must update every consuming
//! preview hierarchy without replacing its root or source stage.

use bevy::prelude::*;
use lunco_doc_bevy::DocumentRegistry;
use lunco_usd_bevy::UsdVisualPlugin;
use lunco_usd_bevy_runtime_core::UsdSceneRuntimePlugin;
use lunco_usd_bevy_scene::UsdPrimPath;
use lunco_usd_bevy_stage::UsdStageAsset;
use lunco_usd_commands::UsdCommandsPlugin;
use lunco_usd_core::commands::ApplyUsdOp;
use lunco_usd_document::document::{LayerId, UsdDocument, UsdOp};
use lunco_usd_viewport_core::{
    CloseUsdPreview, FocusUsdPreview, OpenUsdPreview, OpenUsdPreviewView, UsdPreviewId,
    UsdPreviewViewId,
};
use lunco_usd_viewport_runtime::UsdViewportPlugin;

mod support;

/// True when an entity projecting `path` (in any live scene) exists.
fn has_prim_entity(app: &mut App, path: &str) -> bool {
    let mut q = app.world_mut().query::<&UsdPrimPath>();
    q.iter(app.world()).any(|p| p.path == path)
}

/// Count exact children of one explicit preview root.
fn projected_children(app: &mut App, parent: Entity, path: &str) -> usize {
    let mut q = app.world_mut().query::<(&UsdPrimPath, &ChildOf)>();
    q.iter(app.world())
        .filter(|(prim, owner)| prim.path == path && owner.parent() == parent)
        .count()
}

/// Boot a doc-backed viewport app with the twin asset source wired.
fn boot_app() -> App {
    // Inline fixtures resolve through private document mounts.
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    // Twin asset source + `TwinRoots` — must be registered BEFORE `AssetPlugin`
    // (Bevy snapshots asset sources at its build). The doc-backed viewport mounts
    // through the `twin://` source, so the projection path needs it.
    lunco_assets_runtime::register_lunco_asset_sources(&mut app).unwrap();
    app.add_plugins(AssetPlugin::default());
    app.init_asset::<UsdStageAsset>();
    app.init_asset::<Mesh>();
    app.init_asset::<Image>();
    app.add_plugins(UsdVisualPlugin);
    app.add_plugins(UsdCommandsPlugin);
    app.add_plugins(UsdSceneRuntimePlugin);
    app.init_resource::<lunco_core::CommandResults>()
        .init_resource::<lunco_core::ActiveCommandId>();
    app.add_plugins(UsdViewportPlugin);
    app.world_mut()
        .resource_mut::<lunco_render::RenderingQualitySettings>()
        .apply_profile(support::preview_quality());
    app.add_observer(|event: On<lunco_telemetry_core::TelemetryEvent>| {
        if matches!(event.severity, lunco_telemetry_core::Severity::Error) {
            eprintln!("{}: {:?}", event.name, event.data);
        }
    });
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
        doc_id: doc,
        edit_target: LayerId::root(),
    });
    support::settle_visual_projection(&mut app);
    let first_root = app
        .world()
        .resource::<lunco_usd_viewport_core::UsdViewportState>()
        .session(UsdPreviewId(1))
        .unwrap()
        .scene_root();
    let stage = app
        .world()
        .resource::<lunco_usd_viewport_core::UsdViewportState>()
        .session(UsdPreviewId(1))
        .unwrap()
        .stage_handle()
        .id();
    app.world_mut().trigger(OpenUsdPreview {
        preview: UsdPreviewId(2),
        doc_id: doc,
        edit_target: LayerId::root(),
    });
    support::settle_visual_projection(&mut app);
    let second_root = app
        .world()
        .resource::<lunco_usd_viewport_core::UsdViewportState>()
        .session(UsdPreviewId(2))
        .unwrap()
        .scene_root();
    assert_ne!(first_root, second_root);
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
    app.world_mut()
        .resource_mut::<lunco_core::ActiveCommandId>()
        .set(Some(1));
    app.world_mut().trigger(ApplyUsdOp {
        doc_id: doc,
        parent_gen: None,
        op: UsdOp::AddPrim {
            edit_target: LayerId::runtime(),
            parent_path: "/World".into(),
            name: "Box".into(),
            type_name: Some("Cube".into()),
            reference: None,
            reference_prim_path: None,
        },
    });
    app.world_mut()
        .resource_mut::<lunco_core::ActiveCommandId>()
        .set(None);
    app.update();
    let result = app.world().resource::<lunco_core::CommandResults>().get(1);
    assert!(
        matches!(result, Some(lunco_core::CommandOutcome::Succeeded(_))),
        "typed edit failed: {result:?}"
    );

    // The source cursor and every preview hierarchy must consume this edit.
    support::settle_visual_projection(&mut app);
    for root in [first_root, second_root] {
        assert_eq!(projected_children(&mut app, root, "/World/Box"), 1);
    }
    app.world_mut().trigger(ApplyUsdOp {
        doc_id: doc,
        parent_gen: None,
        op: UsdOp::RemovePrim {
            edit_target: LayerId::runtime(),
            path: "/World/Box".into(),
        },
    });
    support::settle_visual_projection(&mut app);
    assert!(!has_prim_entity(&mut app, "/World/Box"));
    let state = app
        .world()
        .resource::<lunco_usd_viewport_core::UsdViewportState>();
    for (preview, root) in [
        (UsdPreviewId(1), first_root),
        (UsdPreviewId(2), second_root),
    ] {
        let session = state.session(preview).unwrap();
        assert_eq!(session.scene_root(), root);
        assert_eq!(session.stage_handle().id(), stage);
    }
}

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
        doc_id: first_doc,
        edit_target: LayerId::root(),
    });
    support::settle_visual_projection(&mut app);
    let first_stage = app
        .world()
        .resource::<lunco_usd_viewport_core::UsdViewportState>()
        .session(UsdPreviewId(1))
        .expect("first assembly has an open preview")
        .stage_handle()
        .id();
    let first_root = app
        .world()
        .resource::<lunco_usd_viewport_core::UsdViewportState>()
        .session(UsdPreviewId(1))
        .unwrap()
        .scene_root();
    assert!(has_prim_entity(&mut app, "/World/First"));

    app.world_mut().trigger(OpenUsdPreviewView {
        preview: UsdPreviewId(1),
        view: UsdPreviewViewId(2),
    });
    app.update();
    let state = app
        .world()
        .resource::<lunco_usd_viewport_core::UsdViewportState>();
    let primary = state
        .focused_view_id()
        .expect("opening a preview creates a primary view");
    let secondary = state
        .view(UsdPreviewViewId(2))
        .expect("additional view is registered");
    assert_eq!(secondary.preview(), UsdPreviewId(1));
    assert_eq!(
        state.session(UsdPreviewId(1)).unwrap().scene_root(),
        first_root
    );
    assert_ne!(state.view(primary).unwrap().camera(), secondary.camera());
    assert_ne!(primary, UsdPreviewViewId(2));
    assert_eq!(state.view_count(), 2);

    app.world_mut().trigger(OpenUsdPreview {
        preview: UsdPreviewId(2),
        doc_id: second_doc,
        edit_target: LayerId::runtime(),
    });
    support::settle_visual_projection(&mut app);
    let state = app
        .world()
        .resource::<lunco_usd_viewport_core::UsdViewportState>();
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
    let state = app
        .world()
        .resource::<lunco_usd_viewport_core::UsdViewportState>();
    assert_eq!(state.focused_doc(), Some(first_doc));
    assert_eq!(state.session_count(), 2);
    assert!(state.session(UsdPreviewId(2)).is_some());

    // Closing one lease removes only its presentation entities and releases
    // no shared document authority still owned by the other lease.
    app.world_mut().trigger(CloseUsdPreview {
        preview: UsdPreviewId(2),
    });
    app.update();
    let state = app
        .world()
        .resource::<lunco_usd_viewport_core::UsdViewportState>();
    assert_eq!(state.session_count(), 1);
    assert!(state.session(UsdPreviewId(1)).is_some());
    assert!(state.session(UsdPreviewId(2)).is_none());
    assert!(app.world().get_entity(second_root).is_err());
    assert!(
        app.world()
            .resource::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
            .coords_of(second_doc)
            .is_none()
    );
    assert!(
        app.world()
            .resource::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
            .coords_of(first_doc)
            .is_some()
    );
}
