//! USD-native co-sim wiring — `connectionPaths` → `SimConnection`.
//!
//! `rewire_usd_connections` rebuilds the derived `SimConnection` set from native
//! `connectionPaths` whenever prim entities spawn/despawn (structural) or a
//! connection edit is drained (`WiringDirty`). These tests cover: the reader
//! (`UsdRead::connections`, all-sources), derivation-at-load through the real
//! system, the SSP factor/offset transform, and that every migrated asset's
//! `.connect` authoring reads back the exact edges the old `lunco:simWires` /
//! wire-prims encoded. The wiring is a **pure derived cache** of USD.

use bevy::asset::AssetApp;
use bevy::prelude::*;
use lunco_cosim::SimConnection;
use lunco_usd_bevy::{CanonicalStages, StageRecipe, UsdPrimPath, UsdRead, UsdStageAsset};
use lunco_usd_sim::cosim::{rewire_usd_connections, WiringDirty};
use openusd::sdf::Path as SdfPath;

const SCENE: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\n\
     def Xform \"World\"\n{\n    def Cube \"Src\"\n    {\n    }\n    def Cube \"Sink\"\n    {\n    }\n}\n";

/// Build an app with a live canonical stage for `SCENE`, initial changes drained.
fn setup() -> (App, AssetId<UsdStageAsset>, Handle<UsdStageAsset>) {
    let mut app = App::new();
    app.add_plugins(bevy::asset::AssetPlugin::default())
        .init_asset::<UsdStageAsset>()
        .init_non_send_resource::<CanonicalStages>()
        .init_resource::<WiringDirty>();

    let recipe = StageRecipe::from_source("scene.usda", SCENE);
    let handle = app
        .world_mut()
        .resource_mut::<Assets<UsdStageAsset>>()
        .add(UsdStageAsset {
            recipe: Some(recipe.clone()),
        });
    let id = handle.id();

    app.world_mut()
        .non_send_resource_mut::<CanonicalStages>()
        .get_or_build(id, &recipe)
        .expect("canonical stage builds from the recipe");
    app.world_mut()
        .non_send_resource_mut::<CanonicalStages>()
        .drain_all_changes();
    (app, id, handle)
}

/// End-to-end through the real `rewire_usd_connections` system: spawning the
/// endpoint prims (a **structural** change, exactly as the initial scene load
/// does) derives one `SimConnection` from the authored `connectionPaths` — the
/// path the earlier sink-drain-only design missed at load. Clearing the
/// connection + marking `WiringDirty` rebuilds to zero edges.
#[test]
fn rewire_derives_at_load_and_clears() {
    let (mut app, id, handle) = setup();
    app.add_systems(Update, rewire_usd_connections);

    // Author the connection ONTO THE LIVE STAGE (as `UsdOp::SetConnection` would).
    app.world()
        .non_send_resource::<CanonicalStages>()
        .get(id)
        .unwrap()
        .stage()
        .create_attribute("/World/Sink.inputs:force_y", "float")
        .unwrap()
        .set_connections([SdfPath::new("/World/Src.outputs:netForce").unwrap()])
        .unwrap();

    // Spawn the two prims' entities — a structural change (`Added<UsdPrimPath>`)
    // that triggers the rewire, just like the load-time reconcile spawning them.
    let src = app
        .world_mut()
        .spawn(UsdPrimPath {
            stage_handle: handle.clone(),
            path: "/World/Src".into(),
        })
        .id();
    let sink = app
        .world_mut()
        .spawn(UsdPrimPath {
            stage_handle: handle.clone(),
            path: "/World/Sink".into(),
        })
        .id();

    app.update(); // rewire runs: Added is non-empty → full rebuild derives the edge

    let edges: Vec<SimConnection> = {
        let mut q = app.world_mut().query::<&SimConnection>();
        q.iter(app.world()).cloned().collect()
    };
    assert_eq!(
        edges.len(),
        1,
        "one SimConnection derived at load, got {edges:?}"
    );
    let e = &edges[0];
    assert_eq!(
        e.start_element, src,
        "source entity resolved from /World/Src"
    );
    assert_eq!(
        e.start_connector, "netForce",
        "connector = attr leaf minus `outputs:`"
    );
    assert_eq!(e.end_element, sink, "sink entity resolved from /World/Sink");
    assert_eq!(
        e.end_connector, "force_y",
        "connector = attr leaf minus `inputs:`"
    );

    // Clear the connection → mark dirty (a live edit is not a structural change)
    // → rebuild drops the edge.
    app.world()
        .non_send_resource::<CanonicalStages>()
        .get(id)
        .unwrap()
        .stage()
        .prim(SdfPath::new("/World/Sink").unwrap())
        .attribute("inputs:force_y")
        .set_connections(Vec::<SdfPath>::new())
        .unwrap();
    app.world_mut().resource_mut::<WiringDirty>().0 = true;
    app.update();

    let remaining = {
        let mut q = app.world_mut().query::<&SimConnection>();
        q.iter(app.world()).count()
    };
    assert_eq!(
        remaining, 0,
        "clearing connectionPaths rebuilds to zero edges"
    );
}

// ── Migrated-asset wiring — the `.connect` authoring parses and reads back the
//    exact edges the old `lunco:simWires` / wire-prims encoded (P1.3/P1.4). ────

fn asset_src(rel: &str) -> String {
    let p = format!("{}/../../assets/{}", env!("CARGO_MANIFEST_DIR"), rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {p}: {e}"))
}

/// Build a live canonical stage from raw `.usda` source.
fn build_from_source(src: &str) -> (App, AssetId<UsdStageAsset>) {
    let mut app = App::new();
    app.add_plugins(bevy::asset::AssetPlugin::default())
        .init_asset::<UsdStageAsset>()
        .init_non_send_resource::<CanonicalStages>();
    let recipe = StageRecipe::from_source("asset.usda", src);
    let handle = app
        .world_mut()
        .resource_mut::<Assets<UsdStageAsset>>()
        .add(UsdStageAsset {
            recipe: Some(recipe.clone()),
        });
    let id = handle.id();
    app.world_mut()
        .non_send_resource_mut::<CanonicalStages>()
        .get_or_build(id, &recipe)
        .expect("migrated asset must build (valid .connect syntax)");
    (app, id)
}

/// The composed connection sources of `prim.attr` on the built stage.
fn conns(app: &App, id: AssetId<UsdStageAsset>, prim: &str, attr: &str) -> Vec<String> {
    let stages = app.world().non_send_resource::<CanonicalStages>();
    let cs = stages.get(id).expect("stage present");
    cs.view().connections(&SdfPath::new(prim).unwrap(), attr)
}

#[test]
fn python_balloon_asset_wiring_migrated() {
    let (app, id) = build_from_source(&asset_src("vessels/balloons/python_balloon.usda"));
    assert_eq!(
        conns(&app, id, "/PythonBalloon", "inputs:force_y"),
        ["/PythonBalloon.outputs:netForce"]
    );
    assert_eq!(
        conns(&app, id, "/PythonBalloon", "inputs:height"),
        ["/PythonBalloon.outputs:height"]
    );
    assert_eq!(
        conns(&app, id, "/PythonBalloon", "inputs:velocity"),
        ["/PythonBalloon.outputs:velocity_y"]
    );
}

#[test]
fn modelica_balloon_asset_wiring_migrated() {
    let (app, id) = build_from_source(&asset_src("vessels/balloons/modelica_balloon.usda"));
    assert_eq!(
        conns(&app, id, "/ModelicaBalloon", "inputs:force_y"),
        ["/ModelicaBalloon.outputs:netForce"]
    );
    assert_eq!(
        conns(&app, id, "/ModelicaBalloon", "inputs:collider"),
        ["/ModelicaBalloon.outputs:volume"]
    );
    assert_eq!(
        conns(&app, id, "/ModelicaBalloon", "inputs:height"),
        ["/ModelicaBalloon.outputs:height"]
    );
    assert_eq!(
        conns(&app, id, "/ModelicaBalloon", "inputs:velocity"),
        ["/ModelicaBalloon.outputs:velocity_y"]
    );
}

#[test]
fn sun_tracker_asset_wiring_migrated() {
    let (app, id) = build_from_source(&asset_src("scenes/sandbox/sun_tracker_test.usda"));
    // Self-loop on the controller + cross-prim edge onto the hinge.
    assert_eq!(
        conns(
            &app,
            id,
            "/SunTrackerTest/SolarTower/Controller",
            "inputs:sun_azimuth"
        ),
        ["/SunTrackerTest/SolarTower/Controller.outputs:sun_azimuth"]
    );
    assert_eq!(
        conns(&app, id, "/SunTrackerTest/SolarTower/Hinge", "inputs:angle"),
        ["/SunTrackerTest/SolarTower/Controller.outputs:yaw"]
    );
}

#[test]
fn sandbox_scene_asset_wiring_migrated() {
    let (app, id) = build_from_source(&asset_src("scenes/sandbox/sandbox_scene.usda"));
    assert_eq!(
        conns(&app, id, "/SandboxScene/Amplifier", "inputs:signal"),
        ["/SandboxScene/Oscillator.outputs:signal"]
    );
    assert_eq!(
        conns(&app, id, "/SandboxScene/CosimTarget", "inputs:force_y"),
        ["/SandboxScene/Amplifier.outputs:scaled"]
    );
}

/// The lander is a REFERENCED vessel asset (`vessels/landers/descent_lander.usda`),
/// so its wiring only exists once the reference arc is composed. `build_from_source`
/// builds a lone in-memory layer and cannot resolve `@../../vessels/...@`; compose
/// the file with its real layer closure instead. This is what proves the asset-local
/// `.connect` targets (`</DescentLander.outputs:force_y>`) rebase onto the scene
/// prim (`/LanderTest/Lander`) through the arc.
#[test]
fn lander_asset_wiring_migrated() {
    let scene = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/scenes/sandbox/lander_test.usda");
    let stage = lunco_usd_bevy::compose_file_to_stage(&scene).expect("compose lander_test.usda");
    let view = lunco_usd_bevy::StageView::new(&stage);
    let lander = SdfPath::new("/LanderTest/Lander").unwrap();
    let conns = |attr: &str| view.connections(&lander, attr);

    // A representative sample of the 17 self-loops + the cross-prim altimeter edge.
    assert_eq!(conns("inputs:force_y"), ["/LanderTest/Lander.outputs:force_y"]);
    assert_eq!(conns("inputs:q_w"), ["/LanderTest/Lander.outputs:quat_w"]);
    assert_eq!(
        conns("inputs:descent_rate"),
        ["/LanderTest/Lander.outputs:velocity_y"]
    );
    assert_eq!(
        conns("inputs:altitude"),
        ["/LanderTest/Lander/Altimeter.outputs:range"]
    );
}

// ── P1.2b: SSP LinearTransformation (factor/offset) on the sink port. ─────────

/// `double lunco:factor:<port>` / `lunco:offset:<port>` on the sink prim are read
/// into the derived `SimConnection` (propagated value = `src * factor + offset`).
#[test]
fn rewire_applies_factor_and_offset() {
    let (mut app, id, handle) = setup();
    app.add_systems(Update, rewire_usd_connections);

    {
        let stages = app.world().non_send_resource::<CanonicalStages>();
        let stage = stages.get(id).unwrap().stage();
        stage
            .create_attribute("/World/Sink.inputs:force_y", "float")
            .unwrap()
            .set_connections([SdfPath::new("/World/Src.outputs:netForce").unwrap()])
            .unwrap();
        stage
            .create_attribute("/World/Sink.lunco:factor:force_y", "double")
            .unwrap()
            .set(openusd::sdf::Value::Double(2.5))
            .unwrap();
        stage
            .create_attribute("/World/Sink.lunco:offset:force_y", "double")
            .unwrap()
            .set(openusd::sdf::Value::Double(0.5))
            .unwrap();
    }

    app.world_mut().spawn(UsdPrimPath {
        stage_handle: handle.clone(),
        path: "/World/Src".into(),
    });
    app.world_mut().spawn(UsdPrimPath {
        stage_handle: handle.clone(),
        path: "/World/Sink".into(),
    });
    app.update();

    let edges: Vec<SimConnection> = {
        let mut q = app.world_mut().query::<&SimConnection>();
        q.iter(app.world()).cloned().collect()
    };
    assert_eq!(edges.len(), 1, "one edge derived, got {edges:?}");
    assert_eq!(edges[0].scale, 2.5, "factor read from lunco:factor:force_y");
    assert_eq!(
        edges[0].offset, 0.5,
        "offset read from lunco:offset:force_y"
    );
}

/// A transform authored as `float` (matching the `float`-typed port it scales, as
/// a real asset naturally would) must still be read — a strict `double` read would
/// silently drop it and apply identity (1, 0), a wrong-magnitude physics bug.
#[test]
fn rewire_reads_float_authored_transform() {
    let (mut app, id, handle) = setup();
    app.add_systems(Update, rewire_usd_connections);

    {
        let stages = app.world().non_send_resource::<CanonicalStages>();
        let stage = stages.get(id).unwrap().stage();
        stage
            .create_attribute("/World/Sink.inputs:force_y", "float")
            .unwrap()
            .set_connections([SdfPath::new("/World/Src.outputs:netForce").unwrap()])
            .unwrap();
        stage
            .create_attribute("/World/Sink.lunco:factor:force_y", "float")
            .unwrap()
            .set(openusd::sdf::Value::Float(2.5))
            .unwrap();
        stage
            .create_attribute("/World/Sink.lunco:offset:force_y", "float")
            .unwrap()
            .set(openusd::sdf::Value::Float(0.5))
            .unwrap();
    }

    app.world_mut().spawn(UsdPrimPath {
        stage_handle: handle.clone(),
        path: "/World/Src".into(),
    });
    app.world_mut().spawn(UsdPrimPath {
        stage_handle: handle.clone(),
        path: "/World/Sink".into(),
    });
    app.update();

    let edges: Vec<SimConnection> = {
        let mut q = app.world_mut().query::<&SimConnection>();
        q.iter(app.world()).cloned().collect()
    };
    assert_eq!(edges.len(), 1, "one edge derived, got {edges:?}");
    assert_eq!(
        edges[0].scale, 2.5,
        "float-authored factor must not fall back to identity"
    );
    assert_eq!(
        edges[0].offset, 0.5,
        "float-authored offset must not fall back to identity"
    );
}
