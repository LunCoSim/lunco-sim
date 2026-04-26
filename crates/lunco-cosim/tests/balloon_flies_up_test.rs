//! End-to-end physics test: does the Modelica balloon actually fly up?
//!
//! Stronger assertion than `balloon_cosim_test`. That test verifies the
//! cosim *signal chain* (Modelica → SimComponent → AvianSim.inputs) but
//! deliberately skips `PhysicsPlugins`, so motion is never integrated.
//! This file *does* run Avian's solver headlessly so we can assert that
//! `Position.y` actually increases — which is what the user observes
//! (or doesn't, hence the bug).
//!
//! Setup mirrors `rover_sandbox_usd`:
//!   - `Gravity::ZERO` — Modelica's `netForce` already excludes weight.
//!   - `Time::<Fixed>::from_hz(60.0)`.
//!   - Avian `RigidBody::Dynamic` + `Mass(4.5)` + `Collider::sphere(1.0)`.
//!   - `BalloonModelMarker` so the same `compile_balloon_model` /
//!     `setup_balloon_wires` shape that runs in production fires here.
//!
//! The miniature pipeline systems are duplicated from `balloon_cosim_test`
//! since both live in `tests/` (separate crate compilation units).

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use std::time::Duration;

use avian3d::prelude::*;
use lunco_cosim::{CoSimPlugin, SimComponent, SimConnection};
use lunco_modelica::{
    extract_inputs_with_defaults, extract_model_name, extract_parameters, ModelicaChannels,
    ModelicaCommand, ModelicaCorePlugin, ModelicaModel,
};
use lunco_sandbox_edit::catalog::BalloonModelMarker;

const BALLOON_MO: &str = include_str!("../../../assets/models/Balloon.mo");

// ─── Miniature pipeline (matches the production binary) ─────────────────────

fn compile_balloon_model(
    mut commands: Commands,
    q_new: Query<(Entity, &Name), Added<BalloonModelMarker>>,
    channels: Res<ModelicaChannels>,
) {
    for (entity, name) in &q_new {
        let source = BALLOON_MO.to_string();
        let model_name = extract_model_name(&source).unwrap_or_else(|| "Balloon".into());
        let params = extract_parameters(&source);
        let inputs = extract_inputs_with_defaults(&source);

        commands.entity(entity).insert(ModelicaModel {
            model_path: std::path::PathBuf::from("balloon.mo"),
            model_name: model_name.clone(),
            parameters: params,
            inputs: inputs.into_iter().collect(),
            ..default()
        });

        let _ = channels.tx.send(ModelicaCommand::Compile {
            entity,
            session_id: 0,
            model_name,
            source,
            stream: None,
        });

        eprintln!("test: dispatched Modelica Compile for '{name}'");
    }
}

fn setup_balloon_wires(
    mut commands: Commands,
    q_new: Query<(Entity, &Name, &ModelicaModel), (With<BalloonModelMarker>, Without<SimComponent>)>,
) {
    for (entity, name, model) in &q_new {
        if model.variables.is_empty() {
            continue;
        }
        eprintln!(
            "test: '{name}' compiled — variables: {:?}",
            model.variables.keys().collect::<Vec<_>>()
        );

        commands.entity(entity).insert(SimComponent {
            model_name: model.model_name.clone(),
            parameters: model.parameters.clone(),
            inputs: model.inputs.clone(),
            outputs: model.variables.clone(),
            ..default()
        });

        commands.spawn(SimConnection {
            start_element: entity, start_connector: "netForce".into(),
            end_element: entity,   end_connector: "force_y".into(), scale: 1.0,
        });
        commands.spawn(SimConnection {
            start_element: entity, start_connector: "height".into(),
            end_element: entity,   end_connector: "height".into(), scale: 1.0,
        });
        commands.spawn(SimConnection {
            start_element: entity, start_connector: "velocity_y".into(),
            end_element: entity,   end_connector: "velocity".into(), scale: 1.0,
        });

        commands.entity(entity).remove::<BalloonModelMarker>();
    }
}

fn sync_modelica_outputs(
    mut q_models: Query<(&ModelicaModel, &mut SimComponent), Without<BalloonModelMarker>>,
) {
    for (model, mut comp) in &mut q_models {
        for (name, val) in &model.variables {
            comp.outputs.insert(name.clone(), *val);
        }
    }
}

fn sync_inputs_to_modelica(
    mut q_models: Query<(&SimComponent, &mut ModelicaModel), Without<BalloonModelMarker>>,
) {
    for (comp, mut model) in &mut q_models {
        for (name, val) in &comp.inputs {
            model.inputs.insert(name.clone(), *val);
        }
    }
}

// ─── The test ───────────────────────────────────────────────────────────────

#[test]
fn balloon_flies_up_under_buoyancy() {
    let mut app = App::new();

    app.add_plugins((
        MinimalPlugins,
        TransformPlugin,
        bevy::asset::AssetPlugin::default(),
        bevy::mesh::MeshPlugin,
        PhysicsPlugins::default(),
    ))
    .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(1.0 / 60.0)))
    .insert_resource(Gravity::ZERO)
    .insert_resource(Time::<Fixed>::from_hz(60.0));

    app.add_plugins((CoSimPlugin, ModelicaCorePlugin));

    app.add_systems(
        Update,
        (
            compile_balloon_model,
            setup_balloon_wires,
            sync_modelica_outputs,
            sync_inputs_to_modelica,
        ),
    );

    app.finish();

    let initial_y = 5.0_f64;
    let balloon = app.world_mut().spawn((
        Name::new("Test Balloon"),
        Transform::from_xyz(0.0, initial_y as f32, 0.0),
        RigidBody::Dynamic,
        Collider::sphere(1.0),
        Mass(4.5),
        BalloonModelMarker::default(),
    )).id();

    // Wait for Modelica compile + setup_balloon_wires.
    let mut compiled = false;
    for i in 0..600 {
        app.update();
        if app.world().get::<SimComponent>(balloon).is_some() {
            eprintln!("test: SimComponent ready after {} ticks", i + 1);
            compiled = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(compiled, "balloon never received SimComponent — Modelica compile failed?");

    // Snapshot starting Y, then run physics for ~2 simulated seconds (120 ticks).
    let start_y = app.world().get::<Position>(balloon).map(|p| p.0.y).unwrap_or(initial_y);
    eprintln!("test: starting Position.y = {start_y}");

    let mut max_force_y = 0.0_f64;
    let mut max_velocity_y = 0.0_f64;
    for _ in 0..240 {
        app.update();
        std::thread::sleep(Duration::from_millis(2));

        if let Some(comp) = app.world().get::<SimComponent>(balloon) {
            if let Some(&fy) = comp.inputs.get("force_y") {
                if fy.abs() > max_force_y.abs() { max_force_y = fy; }
            }
        }
        if let Some(v) = app.world().get::<LinearVelocity>(balloon) {
            if v.0.y.abs() > max_velocity_y.abs() { max_velocity_y = v.0.y; }
        }
    }

    let end_y = app.world().get::<Position>(balloon).map(|p| p.0.y).unwrap_or(start_y);
    let netforce = app.world().get::<SimComponent>(balloon)
        .and_then(|c| c.outputs.get("netForce").copied())
        .unwrap_or(f64::NAN);

    eprintln!("test: ending Position.y = {end_y}");
    eprintln!("test: max Modelica netForce = {netforce}");
    eprintln!("test: max force_y propagated = {max_force_y}");
    eprintln!("test: max LinearVelocity.y = {max_velocity_y}");

    assert!(
        netforce.is_finite() && netforce > 0.0,
        "Modelica netForce should be positive (buoyancy) but was {netforce}"
    );
    assert!(
        max_force_y.abs() > 0.1,
        "force_y never propagated through SimConnection — wires broken?"
    );
    assert!(
        end_y > start_y + 0.1,
        "balloon did not move upward: start={start_y} end={end_y} (Δ={:.4} m)",
        end_y - start_y
    );
}

/// Production-mirroring test: real flat gravity (4.5 kg × 9.81 = 44.1 N down)
/// fights Modelica buoyancy (~48 N up). Net force is small but positive,
/// balloon should still rise. This catches the case where the user reports
/// "doesn't fly up" — likely because gravity dominates while Modelica is
/// still compiling, the balloon falls below ground / sleeps, and never
/// recovers.
#[test]
fn balloon_flies_up_with_flat_gravity() {
    use lunco_celestial::{Gravity as CelestialGravity, GravityPlugin};
    use lunco_environment::EnvironmentPlugin;

    let mut app = App::new();

    app.add_plugins((
        MinimalPlugins,
        TransformPlugin,
        bevy::asset::AssetPlugin::default(),
        bevy::mesh::MeshPlugin,
        PhysicsPlugins::default(),
    ))
    .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(1.0 / 60.0)))
    .insert_resource(Gravity::ZERO)
    .insert_resource(CelestialGravity::flat(9.81, bevy::math::DVec3::NEG_Y))
    .insert_resource(Time::<Fixed>::from_hz(60.0));

    app.add_plugins((CoSimPlugin, ModelicaCorePlugin, GravityPlugin, EnvironmentPlugin));

    app.add_systems(
        Update,
        (
            compile_balloon_model,
            setup_balloon_wires,
            sync_modelica_outputs,
            sync_inputs_to_modelica,
        ),
    );

    app.finish();

    // Static ground at y=0 (matches `sandbox_scene.usda`'s ground plane).
    // Without this the test passes trivially (no floor to land on) and the
    // production failure mode — balloon falls during compile, hits ground,
    // gets pinned — never reproduces.
    app.world_mut().spawn((
        Name::new("Ground"),
        Transform::from_xyz(0.0, -0.1, 0.0),
        RigidBody::Static,
        Collider::cuboid(1000.0, 0.2, 1000.0),
    ));

    let initial_y = 5.0_f64;
    let balloon = app.world_mut().spawn((
        Name::new("Test Balloon (with gravity)"),
        Transform::from_xyz(0.0, initial_y as f32, 0.0),
        RigidBody::Dynamic,
        Collider::sphere(1.0),
        Mass(4.5),
        BalloonModelMarker::default(),
    )).id();

    // Same compile-wait as the no-gravity test.
    let mut compiled = false;
    for _ in 0..600 {
        app.update();
        if app.world().get::<SimComponent>(balloon).is_some() { compiled = true; break; }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(compiled, "balloon never compiled");

    let start_y = app.world().get::<Position>(balloon).map(|p| p.0.y).unwrap_or(initial_y);
    eprintln!("test (gravity): starting Position.y = {start_y}");

    let mut min_y = start_y;
    for _ in 0..480 {  // 8 simulated seconds — give it time to fall AND recover
        app.update();
        std::thread::sleep(Duration::from_millis(2));
        if let Some(p) = app.world().get::<Position>(balloon) {
            if p.0.y < min_y { min_y = p.0.y; }
        }
    }

    let end_y = app.world().get::<Position>(balloon).map(|p| p.0.y).unwrap_or(start_y);
    let netforce = app.world().get::<SimComponent>(balloon)
        .and_then(|c| c.outputs.get("netForce").copied())
        .unwrap_or(f64::NAN);
    let (force_y_in, total_force_in) = app.world().get::<SimComponent>(balloon)
        .map(|c| (
            c.inputs.get("force_y").copied().unwrap_or(0.0),
            c.inputs.iter().map(|(k, v)| format!("{k}={v:.2}")).collect::<Vec<_>>().join(", ")
        ))
        .unwrap_or((0.0, String::new()));

    eprintln!("test (gravity): start={start_y} min={min_y} end={end_y}");
    eprintln!("test (gravity): netForce={netforce} force_y_input={force_y_in}");
    eprintln!("test (gravity): all SimComponent.inputs: {total_force_in}");

    assert!(
        end_y > start_y + 0.05,
        "balloon should still rise against flat gravity (buoyancy {netforce:.1} N > weight {:.1} N), \
         but ended at y={end_y:.3} from start={start_y:.3} (min reached {min_y:.3})",
        4.5_f64 * 9.81
    );
}
