//! Cross-entity cosim chain regression test.
//!
//! Wires three engines on three different entities:
//!
//!   Modelica oscillator (entity A)  — outputs `signal = sin(2π·1Hz·t)`
//!         │  SimConnection { A.signal → B.signal }
//!         ▼
//!   Python amplifier (entity B)     — outputs `scaled = signal × 50`
//!         │  SimConnection { B.scaled → C.force_y }
//!         ▼
//!   Avian sphere     (entity C)     — RigidBody::Dynamic, receives force
//!
//! Asserts each link in the chain carries data:
//!   1. Modelica step produces a finite, oscillating `signal`.
//!   2. Python step reads `signal` and writes `scaled` ≈ signal × 50.
//!   3. AvianSim integrates the propagated `force_y` into LinearVelocity.
//!
//! The test bypasses USD asset loading (which needs the full Bevy asset
//! pipeline) and constructs entities + wires directly, mirroring what
//! `lunco_usd_sim::cosim::process_usd_cosim_wires` would emit at
//! runtime. The USD reader path is exercised live by the
//! sandbox scene's CosimChain demo.

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use std::time::Duration;

use avian3d::prelude::*;
use lunco_cosim::{CoSimPlugin, SimComponent, SimConnection, SimStatus};
use lunco_doc::{DocumentHost, DocumentId};
use lunco_modelica::{
    extract_inputs_with_defaults, extract_model_name, extract_parameters, ModelicaChannels,
    ModelicaCommand, ModelicaCorePlugin, ModelicaModel,
};
use lunco_scripting::{
    doc::{ScriptDocument, ScriptLanguage, ScriptedModel},
    LunCoScriptingPlugin, ScriptRegistry,
};

const OSCILLATOR_MO: &str = include_str!("../../../assets/models/Oscillator.mo");
const AMPLIFIER_PY: &str = include_str!("../../../assets/models/Amplifier.py");

fn wrap_modelica_into_simcomponent(
    mut commands: Commands,
    q_new: Query<(Entity, &ModelicaModel), Without<SimComponent>>,
) {
    for (entity, model) in q_new.iter() {
        if model.variables.is_empty() { continue; }
        commands.entity(entity).insert(SimComponent {
            model_name: model.model_name.clone(),
            parameters: model.parameters.clone(),
            inputs: model.inputs.clone(),
            outputs: model.variables.clone(),
            status: SimStatus::Running,
            is_stepping: model.is_stepping,
        });
    }
}

fn sync_modelica_outputs(mut q: Query<(&ModelicaModel, &mut SimComponent)>) {
    for (model, mut comp) in &mut q {
        for (name, val) in &model.variables {
            comp.outputs.insert(name.clone(), *val);
        }
    }
}

fn sync_modelica_inputs(mut q: Query<(&SimComponent, &mut ModelicaModel)>) {
    for (comp, mut model) in &mut q {
        for (name, val) in &comp.inputs {
            model.inputs.insert(name.clone(), *val);
        }
    }
}

fn sync_script_outputs(mut q: Query<(&ScriptedModel, &mut SimComponent)>) {
    for (model, mut comp) in &mut q {
        for (name, val) in &model.outputs {
            comp.outputs.insert(name.clone(), *val);
        }
    }
}

fn sync_script_inputs(mut q: Query<(&SimComponent, &mut ScriptedModel)>) {
    for (comp, mut model) in &mut q {
        for (name, val) in &comp.inputs {
            model.inputs.insert(name.to_string(), *val);
        }
    }
}

#[test]
fn cosim_chain_modelica_python_avian_propagates_data() {
    // Enable info-level logs so the test surfaces Modelica worker
    // diagnostics on failure. Best-effort — ignored if a logger is
    // already installed (e.g. parallel test).
    let _ = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,cranelift=warn,diffsol=warn"),
    )
    .is_test(true)
    .try_init();

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

    app.add_plugins((CoSimPlugin, ModelicaCorePlugin, LunCoScriptingPlugin));

    app.add_systems(
        Update,
        (
            wrap_modelica_into_simcomponent,
            sync_modelica_outputs,
            sync_modelica_inputs,
            sync_script_outputs,
            sync_script_inputs,
        ),
    );

    app.finish();

    // ── Spawn the three nodes ───────────────────────────────────────
    // A: Modelica oscillator
    let oscillator = app.world_mut().spawn(Name::new("Oscillator")).id();
    // B: Python amplifier
    let amplifier = app.world_mut().spawn(Name::new("Amplifier")).id();
    // C: Avian sphere (target)
    let target = app.world_mut().spawn((
        Name::new("CosimTarget"),
        Transform::from_xyz(0.0, 5.0, 0.0),
        RigidBody::Dynamic,
        Collider::sphere(0.5),
        Mass(1.0),
    )).id();

    // ── Cross-entity wires ──────────────────────────────────────────
    app.world_mut().spawn(SimConnection {
        start_element: oscillator, start_connector: "signal".into(),
        end_element:   amplifier,  end_connector:   "signal".into(),
        scale: 1.0,
    });
    app.world_mut().spawn(SimConnection {
        start_element: amplifier, start_connector: "scaled".into(),
        end_element:   target,    end_connector:   "force_y".into(),
        scale: 1.0,
    });

    // ── Wire engines to nodes ───────────────────────────────────────
    // Modelica oscillator: insert ModelicaModel + dispatch Compile.
    {
        let model_name = extract_model_name(OSCILLATOR_MO).unwrap_or_else(|| "Oscillator".into());
        let parameters = extract_parameters(OSCILLATOR_MO);
        let inputs = extract_inputs_with_defaults(OSCILLATOR_MO).into_iter().collect();
        app.world_mut().entity_mut(oscillator).insert(ModelicaModel {
            model_name: model_name.clone(),
            parameters,
            inputs,
            ..default()
        });
        let tx = app.world().resource::<ModelicaChannels>().tx.clone();
        let _ = tx.send(ModelicaCommand::Compile {
            entity: oscillator,
            session_id: 0,
            model_name,
            source: OSCILLATOR_MO.to_string(),
            stream: None,
        });
    }

    // Python amplifier — register doc + insert ScriptedModel + SimComponent.
    {
        let world = app.world_mut();
        let doc_id = DocumentId::new(amplifier.index().index() as u64 + 10_000);
        world.resource_mut::<ScriptRegistry>().documents.insert(
            doc_id,
            DocumentHost::new(ScriptDocument {
                id: doc_id.raw(),
                generation: 0,
                language: ScriptLanguage::Python,
                source: AMPLIFIER_PY.to_string(),
                inputs: vec!["signal".to_string()],
                outputs: vec!["scaled".to_string()],
            }),
        );
        world.entity_mut(amplifier).insert((
            ScriptedModel {
                document_id: Some(doc_id.raw()),
                language: Some(ScriptLanguage::Python),
                paused: false,
                inputs: Default::default(),
                outputs: Default::default(),
            },
            SimComponent {
                model_name: "Amplifier".into(),
                status: SimStatus::Running,
                ..default()
            },
        ));
    }

    // ── Wait for Modelica to compile ────────────────────────────────
    let mut compiled = false;
    for _ in 0..600 {
        app.update();
        if app.world().get::<SimComponent>(oscillator).is_some() { compiled = true; break; }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(compiled, "Oscillator never compiled — Modelica path broken?");

    // ── Step the chain and observe ──────────────────────────────────
    let mut max_signal = 0.0_f64;
    let mut max_scaled = 0.0_f64;
    let mut max_velocity_y = 0.0_f64;

    for _ in 0..240 {
        app.update();
        std::thread::sleep(Duration::from_millis(2));

        if let Some(comp) = app.world().get::<SimComponent>(oscillator) {
            if let Some(&s) = comp.outputs.get("signal") {
                if s.abs() > max_signal.abs() { max_signal = s; }
            }
        }
        if let Some(comp) = app.world().get::<SimComponent>(amplifier) {
            if let Some(&s) = comp.outputs.get("scaled") {
                if s.abs() > max_scaled.abs() { max_scaled = s; }
            }
        }
        if let Some(v) = app.world().get::<LinearVelocity>(target) {
            if v.0.y.abs() > max_velocity_y.abs() { max_velocity_y = v.0.y; }
        }
    }

    eprintln!("test: max |signal|     = {:.4} (Modelica oscillator output)", max_signal.abs());
    eprintln!("test: max |scaled|     = {:.4} (Python amplifier output, expected ≈ signal*50)", max_scaled.abs());
    eprintln!("test: max |velocity_y| = {:.4} (Avian integrated force_y from amplified scale)", max_velocity_y.abs());

    assert!(
        max_signal.abs() > 0.5,
        "Modelica oscillator never produced a signal close to its 1.0 amplitude (max={max_signal})"
    );
    assert!(
        max_scaled.abs() > 25.0,
        "Python amplifier never produced scaled output near gain*amplitude=50 (max={max_scaled}) — \
         likely Python isn't available on this machine, or the Modelica→Python wire isn't propagating."
    );
    assert!(
        max_velocity_y.abs() > 0.5,
        "Avian never integrated the propagated force into target velocity (max={max_velocity_y}) — \
         the Python→Avian wire isn't carrying `scaled` into AvianSim.inputs[\"force_y\"]."
    );
}
