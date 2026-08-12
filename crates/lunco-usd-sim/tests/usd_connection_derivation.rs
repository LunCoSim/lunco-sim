//! USD-native co-sim wiring — `connectionPaths` → `SimConnection`.
//!
//! `rewire_usd_connections` rebuilds the derived `SimConnection` set from native
//! `connectionPaths` whenever prim entities spawn/despawn (structural) or a
//! connection edit is drained (`WiringDirty`). These tests cover: the reader
//! (`UsdRead::connections`, all-sources), derivation-at-load through the real
//! system, the SSP factor/offset transform, and shipped `.connect` authoring.
//! The wiring is a **pure derived cache** of USD.

use bevy::asset::AssetApp;
use bevy::prelude::*;
use lunco_cosim::SimConnection;
use lunco_usd_bevy::{CanonicalStages, StageRecipe, UsdPrimPath, UsdRead, UsdStageAsset};
use lunco_usd_sim::cosim::{rewire_usd_connections, WiringDirty};
use lunco_usd_sim::domain_projection::{
    ActuatorWrenchSynthesizer, DomainSynthesizer, MemberClasses, SynthContext, SynthOutcome,
};
use openusd::sdf::Path as SdfPath;

const SCENE: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\n\
     def Xform \"World\"\n{\n    def Cube \"Src\"\n    {\n    }\n    def Cube \"Sink\"\n    {\n    }\n}\n";

/// Build an app with a live canonical stage for `SCENE`, initial changes drained.
fn setup() -> (App, AssetId<UsdStageAsset>, Handle<UsdStageAsset>) {
    let mut app = App::new();
    app.add_plugins(bevy::asset::AssetPlugin::default())
        .init_asset::<UsdStageAsset>()
        .init_non_send::<CanonicalStages>()
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
        .non_send_mut::<CanonicalStages>()
        .get_or_build(id, &recipe)
        .expect("canonical stage builds from the recipe");
    app.world_mut()
        .non_send_mut::<CanonicalStages>()
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
        .non_send::<CanonicalStages>()
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
        .spawn((
            UsdPrimPath {
                stage_handle: handle.clone(),
                path: "/World/Src".into(),
            },
            lunco_core::PortSurfaceReady,
        ))
        .id();
    let sink = app
        .world_mut()
        .spawn((
            UsdPrimPath {
                stage_handle: handle,
                path: "/World/Sink".into(),
            },
            lunco_core::PortSurfaceReady,
        ))
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
        .non_send::<CanonicalStages>()
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

// ── Shipped-asset wiring — `.connect` authoring parses and reads back the
//    causal edges each component declares. ────────────────────────────────────

fn asset_src(rel: &str) -> String {
    let p = format!("{}/../../assets/{}", env!("CARGO_MANIFEST_DIR"), rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {p}: {e}"))
}

/// Build a live canonical stage from raw `.usda` source.
fn build_from_source(src: &str) -> (App, AssetId<UsdStageAsset>) {
    let mut app = App::new();
    app.add_plugins(bevy::asset::AssetPlugin::default())
        .init_asset::<UsdStageAsset>()
        .init_non_send::<CanonicalStages>();
    let recipe = StageRecipe::from_source("asset.usda", src);
    let handle = app
        .world_mut()
        .resource_mut::<Assets<UsdStageAsset>>()
        .add(UsdStageAsset {
            recipe: Some(recipe.clone()),
        });
    let id = handle.id();
    app.world_mut()
        .non_send_mut::<CanonicalStages>()
        .get_or_build(id, &recipe)
        .expect("migrated asset must build (valid .connect syntax)");
    (app, id)
}

/// The composed connection sources of `prim.attr` on the built stage.
fn conns(app: &App, id: AssetId<UsdStageAsset>, prim: &str, attr: &str) -> Vec<String> {
    let stages = app.world().non_send::<CanonicalStages>();
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
        ["/PythonBalloon.outputs:position_y"]
    );
    assert_eq!(
        conns(&app, id, "/PythonBalloon", "inputs:velocity"),
        ["/PythonBalloon.outputs:velocity_y"]
    );
}

#[test]
fn modelica_balloon_asset_wiring_migrated() {
    let (app, id) = build_from_source(&asset_src("vessels/balloons/modelica_balloon.usda"));
    // The Modelica program lives on a child `Plant` scope, not on the body
    // (asset restructured in d08b027e): the parent exposes only Avian facts and
    // force sinks, `Plant` declares only Modelica inputs. So the body's force
    // sink is fed BY the child, and the child's inputs are fed by the body —
    // the wire crosses the parent/child boundary in both directions.
    assert_eq!(
        conns(&app, id, "/ModelicaBalloon", "inputs:force_y"),
        ["/ModelicaBalloon/Plant.outputs:netForce"]
    );
    assert!(
        conns(&app, id, "/ModelicaBalloon", "inputs:collider").is_empty(),
        "the collider synchronizer consumes `outputs:volume` directly; `collider` is not a port"
    );
    assert_eq!(
        conns(&app, id, "/ModelicaBalloon/Plant", "inputs:height"),
        ["/ModelicaBalloon.outputs:position_y"]
    );
    assert_eq!(
        conns(&app, id, "/ModelicaBalloon/Plant", "inputs:velocity"),
        ["/ModelicaBalloon.outputs:velocity_y"]
    );
}

#[test]
fn sun_tracker_asset_wiring_migrated() {
    let (app, id) = build_from_source(&asset_src("scenes/tests/sun_tracker.usda"));
    // Explicit environment provider into the controller + controller onto hinge.
    assert_eq!(
        conns(
            &app,
            id,
            "/SunTrackerTest/SolarTower/Controller",
            "inputs:sun_mount_x"
        ),
        ["/SunTrackerTest/SolarTower/Environment.outputs:sun_mount_x"]
    );
    assert_eq!(
        conns(&app, id, "/SunTrackerTest/SolarTower/Hinge", "inputs:angle"),
        ["/SunTrackerTest/SolarTower/Controller.outputs:yaw"]
    );
}

#[test]
fn sandbox_scene_asset_wiring_migrated() {
    let (app, id) = build_from_source(&asset_src("scenes/luncosim/sandbox_scene.usda"));
    assert_eq!(
        conns(&app, id, "/SandboxScene/Amplifier", "inputs:signal"),
        ["/SandboxScene/Oscillator.outputs:signal"]
    );
    assert_eq!(
        conns(&app, id, "/SandboxScene/CosimTarget", "inputs:force_y"),
        ["/SandboxScene/Amplifier.outputs:scaled"]
    );
}

/// The lander in a scene is two arcs: the airframe
/// (`vessels/landers/descent_lander.usda`) and the position-PID guidance component
/// (`components/gnc/position_pid_guidance.usda`). Their wiring only exists once both
/// are composed, and `build_from_source` builds a lone in-memory layer that cannot
/// resolve the asset closure — so compose the scene with its real layer closure.
///
/// This proves both sides of the current contract. Asset-local sensor connections
/// rebase through the two reference arcs, while the mission scene owns the live
/// landing-target and estimator-initialization connections.
#[test]
fn lander_asset_wiring_migrated() {
    let scene = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/scenes/luncosim/lander_ops.usda");
    let stage = lunco_usd_bevy::compose_file_to_stage(&scene).expect("compose lander_ops.usda");
    let view = lunco_usd_bevy::StageView::new(&stage);
    let lander = SdfPath::new("/LanderTest/Lander").unwrap();
    let gnc = SdfPath::new("/LanderTest/Lander/GNC").unwrap();

    // The airframe's local actuator contract: the controller emits normalized
    // throttle and body-frame torque demands, and the USD-composed networks consume
    // those outputs.
    assert_eq!(
        view.connections(
            &SdfPath::new("/LanderTest/Lander/MainPropulsion").unwrap(),
            "inputs:valve_opening"
        ),
        ["/LanderTest/Lander.outputs:throttle"]
    );
    assert_eq!(
        view.connections(
            &SdfPath::new("/LanderTest/Lander/AttitudeActuation").unwrap(),
            "inputs:desired_torque_x"
        ),
        ["/LanderTest/Lander.outputs:torque_x"]
    );
    let attitude_actuation = SdfPath::new("/LanderTest/Lander/AttitudeActuation").unwrap();
    assert_eq!(
        view.text(&attitude_actuation, "lunco:synthesizer")
            .as_deref(),
        Some("actuator-wrench")
    );
    assert!(
        !view.has_prim(&SdfPath::new("/LanderTest/Lander/AttitudeActuation/Allocator").unwrap()),
        "the fixed valve allocator must not survive the geometry-derived projection"
    );
    assert_eq!(
        view.connections(&SdfPath::new("/LanderTest/Lander").unwrap(), "inputs:mass"),
        ["/LanderTest/Lander/MainPropulsion.outputs:vehicle_mass_kg"]
    );
    for (controller_port, source_port) in [
        (
            "inputs:controller_inertia_xx",
            "/LanderTest/Lander/MainPropulsion.outputs:vehicle_inertia_xx_kg_m2",
        ),
        (
            "inputs:controller_inertia_yy",
            "/LanderTest/Lander/MainPropulsion.outputs:vehicle_inertia_yy_kg_m2",
        ),
        (
            "inputs:controller_inertia_zz",
            "/LanderTest/Lander/MainPropulsion.outputs:vehicle_inertia_zz_kg_m2",
        ),
    ] {
        assert_eq!(
            view.connections(&SdfPath::new("/LanderTest/Lander").unwrap(), controller_port),
            [source_port],
            "controller inertia must consume the same live Modelica source as the Avian inertia port"
        );
    }

    // The autopilot, composed on by the SCENE. The airframe authors no descent law and
    // no altitude — it flies what it is told, and this wire is what tells it.
    assert_eq!(
        view.connections(&lander, "inputs:guidance_throttle"),
        ["/LanderTest/Lander/GNC.outputs:throttle_cmd"]
    );

    // The guidance reads Modelica conversions of the airframe's raw Avian
    // observations. The vehicle and mission layers author these edges.
    assert_eq!(
        view.connections(&gnc, "inputs:altimeter_range"),
        ["/LanderTest/Lander/Altimeter/Model.outputs:range_m"]
    );
    assert_eq!(
        view.connections(&gnc, "inputs:altimeter_range_rate"),
        ["/LanderTest/Lander/Altimeter/Model.outputs:range_rate_mps"]
    );
    assert_eq!(
        view.connections(&gnc, "inputs:landing_contact"),
        ["/LanderTest/Lander.outputs:landing_contact"]
    );
    assert_eq!(
        view.connections(&gnc, "inputs:imu_coordinate_accel_local_y"),
        ["/LanderTest/Lander/IMU.outputs:coordinate_accel_local_y"]
    );
    assert_eq!(
        view.connections(
            &SdfPath::new("/LanderTest/Lander/IMU").unwrap(),
            "inputs:raw_acceleration_y"
        ),
        ["/LanderTest/Lander.outputs:acceleration_y"]
    );
    let attitude_reference = SdfPath::new("/LanderTest/Lander/AttitudeReference").unwrap();
    for axis in ["w", "x", "y", "z"] {
        assert_eq!(
            view.connections(&attitude_reference, &format!("inputs:attitude_quat_{axis}")),
            [format!(
                "/LanderTest/Lander/IMU.outputs:attitude_quat_{axis}"
            )],
            "attitude reference must consume the IMU-estimated quaternion"
        );
    }

    // The scene selects the live landing target and initializes the sensor-only
    // estimator from the authored spawn condition.
    assert_eq!(
        view.connections(&gnc, "inputs:target_x"),
        ["/LanderTest/LandingTarget.outputs:position_x"]
    );
    assert_eq!(
        view.connections(&gnc, "inputs:target_y"),
        ["/LanderTest/LandingTarget.outputs:position_y"]
    );
    assert_eq!(
        view.connections(&gnc, "inputs:command_tilt_limit_rad"),
        ["/LanderTest/Lander.inputs:command_tilt_limit_rad"]
    );
    assert!(
        view.connections(&lander, "inputs:inertia_xx").is_empty()
            && view.connections(&lander, "inputs:inertia_yy").is_empty()
            && view.connections(&lander, "inputs:inertia_zz").is_empty(),
        "the removed physical inertia spellings must not remain as writes to the Modelica controller"
    );
    assert!(
        view.connections(&gnc, "inputs:initial_vel_y").is_empty(),
        "initial estimator conditions must not be a live body-state feedback edge"
    );
    assert_eq!(view.value::<f32>(&gnc, "inputs:initial_vel_x"), Some(-5.0));
    assert_eq!(view.value::<f32>(&gnc, "inputs:initial_vel_z"), Some(-5.0));
    assert_eq!(
        view.value::<f32>(&lander, "inputs:touchdown_ground_speed_mps"),
        // The reusable descent airframe deliberately leaves this input
        // unauthored; Lander.mo owns its documented 0.5 m/s semantic default.
        // A composed USD read must therefore report no authored override.
        None
    );
    // The IMU is a measurement conversion, not a second estimator state
    // owner.  Its acceleration inputs are live solved Avian observations; the mission
    // initializes navigation state on GNC above and never mirrors it into the
    // sensor contract.
    let imu = SdfPath::new("/LanderTest/Lander/IMU").unwrap();
    assert_eq!(
        view.connections(&imu, "inputs:raw_acceleration_x"),
        ["/LanderTest/Lander.outputs:acceleration_x"]
    );
    assert_eq!(
        view.connections(&imu, "inputs:raw_acceleration_y"),
        ["/LanderTest/Lander.outputs:acceleration_y"]
    );
    assert_eq!(
        view.connections(&imu, "inputs:raw_acceleration_z"),
        ["/LanderTest/Lander.outputs:acceleration_z"]
    );
    let altimeter = SdfPath::new("/LanderTest/Lander/Altimeter/Model").unwrap();
    assert_eq!(
        view.connections(&altimeter, "inputs:ray_distance_m"),
        ["/LanderTest/Lander/Altimeter.outputs:ray_distance"]
    );
    assert_eq!(
        view.connections(&altimeter, "inputs:ray_hit_valid"),
        ["/LanderTest/Lander/Altimeter.outputs:ray_hit_valid"]
    );
}

#[test]
fn lander_actuator_projection_uses_all_authored_force_geometry() {
    let asset = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/vessels/landers/descent_lander.usda");
    let stage = lunco_usd_bevy::compose_file_to_stage(&asset).expect("compose descent_lander.usda");
    let view = lunco_usd_bevy::StageView::new(&stage);
    let root = SdfPath::new("/DescentLander/AttitudeActuation").unwrap();
    let classes = MemberClasses::default();
    let outcome = ActuatorWrenchSynthesizer
        .synthesize(
            &view,
            &root,
            "DescentLander_AttitudeActuation",
            &SynthContext { classes: &classes },
        )
        .expect("authored RCS geometry must be projectable");
    let SynthOutcome::Ready(plan) = outcome else {
        panic!("lander actuator collection must produce a generated plan");
    };
    assert_eq!(plan.component_paths.len(), 12);
    assert_eq!(plan.outputs.len(), 12);
    assert!(plan.source.contains("LunCo.Actuation.WrenchAllocator"));
    assert!(!plan.source.contains("RcsValveAllocator"));
}

#[test]
fn actuator_collection_keeps_physical_force_wires_outside_modelica_membership() {
    let asset = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/vessels/landers/descent_lander.usda");
    let stage = lunco_usd_bevy::compose_file_to_stage(&asset).expect("compose descent_lander.usda");
    let view = lunco_usd_bevy::StageView::new(&stage);
    let members = lunco_usd_bevy::program::modelica_network_member_paths(&view);

    assert!(
        members.contains("/DescentLander/AttitudePropulsion/RcsPitchPosModel"),
        "the Modelica RCS engine must remain owned by its generated solver"
    );
    assert!(
        !members.contains("/DescentLander/RcsPitchPos"),
        "the physical nozzle must remain a live Avian actuator so its force wire is materialised"
    );
}

/// The airframe ALONE has no autopilot — nothing wires `guidance_throttle`, so it is
/// zero and the lander commands no thrust of its own.
///
/// This is the property the whole split exists for: the vehicle in the palette is a
/// vehicle, not a mission. If a scene ever gets guidance it did not ask for, it will be
/// because someone put it back in the airframe, and this is what will catch them.
#[test]
fn the_airframe_alone_has_no_guidance() {
    let asset = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/vessels/landers/descent_lander.usda");
    let stage = lunco_usd_bevy::compose_file_to_stage(&asset).expect("compose descent_lander.usda");
    let view = lunco_usd_bevy::StageView::new(&stage);
    let lander = SdfPath::new("/DescentLander").unwrap();

    assert!(
        view.connections(&lander, "inputs:guidance_throttle")
            .is_empty(),
        "the airframe must not wire its own guidance — an unpossessed lander that \
         flies itself is a mission, not a vehicle"
    );
    assert!(
        !view.has_prim(&SdfPath::new("/DescentLander/GNC").unwrap()),
        "the airframe must carry no guidance program",
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
        let stages = app.world().non_send::<CanonicalStages>();
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

    app.world_mut().spawn((
        UsdPrimPath {
            stage_handle: handle.clone(),
            path: "/World/Src".into(),
        },
        lunco_core::PortSurfaceReady,
    ));
    app.world_mut().spawn((
        UsdPrimPath {
            stage_handle: handle,
            path: "/World/Sink".into(),
        },
        lunco_core::PortSurfaceReady,
    ));
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
        let stages = app.world().non_send::<CanonicalStages>();
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

    app.world_mut().spawn((
        UsdPrimPath {
            stage_handle: handle.clone(),
            path: "/World/Src".into(),
        },
        lunco_core::PortSurfaceReady,
    ));
    app.world_mut().spawn((
        UsdPrimPath {
            stage_handle: handle,
            path: "/World/Sink".into(),
        },
        lunco_core::PortSurfaceReady,
    ));
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
