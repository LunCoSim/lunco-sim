//! Full-stack cosim integration test: balloon.mo + CoSimPlugin + Modelica worker.
//!
//! Sets up a Bevy App with the co-simulation pipeline running against the real
//! `balloon.mo` asset through the background `modelica_worker` thread, spawns a
//! balloon entity, and runs enough frames for:
//!   1. the worker to compile the model and return its variables,
//!   2. `setup_balloon_wires` to insert `SimComponent` + wires,
//!   3. `sync_modelica_outputs` to populate `SimComponent.outputs["netForce"]`,
//!   4. `propagate_wires` to write `AvianSim.inputs["force_y"]`,
//!   5. `apply_sim_forces` to integrate velocity into `LinearVelocity`.
//!
//! This is the canonical regression test for the balloon co-sim architecture.
//! If it passes, the whole chain works end-to-end — the specific bug from
//! 2026-04 (rumoca eliminating algebraics) will never silently re-appear.
//!
//! We intentionally do NOT pull in `avian3d::PhysicsPlugins` here. That plugin
//! requires bevy_render / bevy_asset / bevy_pbr infrastructure that's painful
//! to stand up headless. Instead we add the avian components directly and
//! assert on `LinearVelocity` — the step that actually moves `Position` is
//! covered by Avian's own tests and by manual verification in the running app.

use bevy::prelude::*;
use bevy::app::ScheduleRunnerPlugin;
use bevy::time::TimePlugin;
use std::time::Duration;

use avian3d::prelude::{RigidBody, Position, LinearVelocity};
use lunco_cosim::{CoSimPlugin, SimComponent, SimConnection, AvianSim};
use lunco_cosim::systems::apply_forces::BalloonVelocity;
use lunco_modelica::{
    ModelicaCorePlugin, ModelicaModel, ModelicaCommand, ModelicaChannels,
    extract_model_name, extract_parameters, extract_inputs_with_defaults,
};
use lunco_sandbox_edit::catalog::BalloonModelMarker;

const BALLOON_MO: &str = include_str!("../../../assets/models/balloon.mo");

// Miniature copies of the production `balloon_setup` systems. They're
// duplicated here because `balloon_setup.rs` lives as a `#[path]`-included
// module inside the `rover_sandbox_usd` binary crate and cannot be imported
// from a library test. Keeping them in sync is a manual regression surface —
// if this test passes and the production binary doesn't, the two are out of
// sync and the production systems need a matching change.

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
            variables: Default::default(),
            current_time: 0.0,
            last_step_time: 0.0,
            session_id: 0,
            paused: false,
            is_stepping: false,
        });

        let _ = channels.tx.send(ModelicaCommand::Compile {
            entity,
            session_id: 0,
            model_name,
            source,
        });

        eprintln!("test: sent Compile for balloon '{name}'");
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
            "test: balloon '{name}' compiled — variables: {:?}",
            model.variables.keys().collect::<Vec<_>>()
        );

        let comp = SimComponent {
            model_name: model.model_name.clone(),
            parameters: model.parameters.clone(),
            inputs: model.inputs.clone(),
            outputs: model.variables.clone(),
            ..default()
        };
        commands.entity(entity).insert(comp);
        commands.entity(entity).insert(BalloonVelocity(Vec3::ZERO));

        commands.spawn(SimConnection {
            start_element: entity, start_connector: "netForce".into(),
            end_element: entity, end_connector: "force_y".into(), scale: 1.0,
        });
        commands.spawn(SimConnection {
            start_element: entity, start_connector: "height".into(),
            end_element: entity, end_connector: "height".into(), scale: 1.0,
        });
        commands.spawn(SimConnection {
            start_element: entity, start_connector: "velocity_y".into(),
            end_element: entity, end_connector: "velocity".into(), scale: 1.0,
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

#[test]
fn balloon_netforce_flows_through_cosim_pipeline() {
    let mut app = App::new();

    app.add_plugins((
        MinimalPlugins
            .set(TimePlugin)
            .set(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(0.016))),
        bevy::asset::AssetPlugin::default(),
    ));

    app.insert_resource(Time::<Fixed>::from_hz(60.0));

    // Co-sim systems (propagate_wires + apply_sim_forces) live in FixedUpdate.
    app.add_plugins(CoSimPlugin);
    // Headless Modelica worker (no UI panels, no bevy_egui).
    app.add_plugins(ModelicaCorePlugin);

    app.add_systems(
        Update,
        (
            compile_balloon_model,
            setup_balloon_wires,
            sync_modelica_outputs,
            sync_inputs_to_modelica,
        ),
    );

    // Spawn balloon with the full component stack that apply_sim_forces expects.
    // We skip PhysicsPlugins, but the components themselves are just data.
    let balloon = app.world_mut().spawn((
        Name::new("Test Balloon"),
        Transform::from_xyz(0.0, 15.0, 0.0),
        Position::from_xyz(0.0, 15.0, 0.0),
        RigidBody::Kinematic,
        LinearVelocity::default(),
        AvianSim::default(),
        BalloonModelMarker::default(),
    )).id();

    // The Modelica worker runs on a separate thread. Loop `app.update()` with
    // a short real-time sleep until the balloon receives its SimComponent
    // (meaning compile result arrived and `setup_balloon_wires` ran), or we
    // hit a timeout.
    let mut compiled = false;
    for i in 0..300 {
        app.update();
        if app.world().get::<SimComponent>(balloon).is_some() {
            eprintln!("test: SimComponent inserted after {} updates", i + 1);
            compiled = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(compiled, "balloon never received a SimComponent within 300 updates");

    // Now keep ticking to let:
    //   - spawn_modelica_requests send Step commands
    //   - handle_modelica_responses update model.variables with fresh outputs
    //   - sync_modelica_outputs copy them to SimComponent.outputs
    //   - propagate_wires write AvianSim.inputs["force_y"]
    //   - apply_sim_forces integrate velocity into LinearVelocity
    //
    // Note: apply_sim_forces calls `avian.take_inputs()`, which drains
    // `AvianSim.inputs["force_y"]` every tick. So we can't sample that field
    // from Update. Instead we sample:
    //   * `SimComponent.outputs["netForce"]` — the persistent Modelica output
    //   * `SimComponent.inputs["force_y"]` — written alongside AvianSim.inputs
    //     by `propagate_wires`, and cleared at the top of the NEXT propagate
    //     (one frame later), so it's observable.
    //   * `LinearVelocity.y` — persistent, accumulates across frames.
    let mut max_abs_vel = 0.0_f64;
    let mut last_netforce = 0.0_f64;
    let mut last_force_y_seen = 0.0_f64;

    for _ in 0..200 {
        app.update();
        std::thread::sleep(Duration::from_millis(5));

        if let Some(comp) = app.world().get::<SimComponent>(balloon) {
            if let Some(&nf) = comp.outputs.get("netForce") {
                if nf.is_finite() {
                    last_netforce = nf;
                }
            }
            if let Some(&fy) = comp.inputs.get("force_y") {
                if fy.abs() > last_force_y_seen.abs() {
                    last_force_y_seen = fy;
                }
            }
        }
        if let Some(v) = app.world().get::<LinearVelocity>(balloon) {
            if v.0.y.abs() > max_abs_vel {
                max_abs_vel = v.0.y.abs();
            }
        }
    }

    // Dump final state for debuggability when the test fails.
    if let Some(comp) = app.world().get::<SimComponent>(balloon) {
        let mut outputs: Vec<(&String, &f64)> = comp.outputs.iter().collect();
        outputs.sort_by(|a, b| a.0.cmp(b.0));
        eprintln!("test: SimComponent.outputs = {:?}", outputs);
        eprintln!("test: SimComponent.inputs  = {:?}", comp.inputs);
    }
    if let Some(model) = app.world().get::<ModelicaModel>(balloon) {
        eprintln!("test: ModelicaModel.paused = {}", model.paused);
        eprintln!("test: ModelicaModel.variables keys = {:?}", model.variables.keys().collect::<Vec<_>>());
    }
    eprintln!("test: last Modelica netForce = {}", last_netforce);
    eprintln!("test: max |force_y| through wire = {}", last_force_y_seen.abs());
    eprintln!("test: max |vel_y| observed   = {}", max_abs_vel);

    // Chain assertions — each one localizes the failure.
    assert!(
        last_netforce.is_finite() && last_netforce > 0.0,
        "balloon netForce should be positive (buoyancy > weight) but was {last_netforce}. \
         If NaN or missing: rumoca failed to return the algebraic. \
         If <= 0: balloon.mo parameters are wrong."
    );
    assert!(
        last_force_y_seen.abs() > 0.1,
        "propagate_wires never wrote a non-zero force_y into SimComponent.inputs. \
         This means the netForce → force_y wire isn't routing correctly."
    );
    assert!(
        max_abs_vel > 0.01,
        "apply_sim_forces never produced a non-zero LinearVelocity.y despite seeing \
         netForce = {last_netforce}. max |vel_y| = {max_abs_vel}"
    );
}
