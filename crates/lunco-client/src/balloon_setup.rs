//! Balloon co-simulation setup — compiles balloon.mo and wires it to AvianSim.
//!
//! When a balloon entity spawns (via SpawnCatalog), it gets a `BalloonModelMarker`.
//! This module picks it up, compiles the Modelica model, and creates wires
//! connecting balloon outputs to AvianSim inputs.

use bevy::prelude::*;
use lunco_assets::assets_dir;
use lunco_cosim::{SimComponent, SimStatus, SimWire};
use lunco_cosim::systems::apply_forces::BalloonVelocity;
use lunco_modelica::{
    ModelicaChannels, ModelicaCommand, ModelicaModel,
    extract_model_name, extract_parameters, extract_inputs_with_defaults,
};
use lunco_sandbox_edit::catalog::BalloonModelMarker;

/// System that triggers Modelica compilation for new balloons.
pub fn compile_balloon_model(
    mut commands: Commands,
    q_new: Query<(Entity, &Name), Added<BalloonModelMarker>>,
    channels: Res<ModelicaChannels>,
) {
    for (entity, name) in &q_new {
        let model_path = assets_dir().join("models/balloon.mo");
        let source = match std::fs::read_to_string(&model_path) {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to read balloon.mo: {e}");
                continue;
            }
        };

        let model_name = extract_model_name(&source).unwrap_or_else(|| "Balloon".into());
        let params = extract_parameters(&source);
        let inputs = extract_inputs_with_defaults(&source);

        // Create ModelicaModel BEFORE compiling — handle_modelica_responses
        // only processes results for entities that already have one.
        commands.entity(entity).insert(ModelicaModel {
            model_path: model_path.clone(),
            model_name: model_name.clone(),
            parameters: params,
            inputs: inputs.into_iter().collect(),
            variables: Default::default(),
            current_time: 0.0,
            last_step_time: 0.0,
            session_id: 0,
            paused: false,
            is_stepping: false,
            original_source: std::sync::Arc::from(source.as_str()),
        });

        let _ = channels.tx.send(ModelicaCommand::Compile {
            entity,
            session_id: 0,
            model_name: model_name.clone(),
            source: source.clone(),
        });

        info!("Balloon '{name}' — compiling Modelica model '{model_name}'");

        let temp_dir = lunco_assets::modelica_dir()
            .join(format!("{}_{}", entity.index(), entity.generation()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let _ = std::fs::write(temp_dir.join("model.mo"), &source);
    }
}

/// System that creates SimComponent and wires after Modelica compilation succeeds.
///
/// Watches for balloon entities with a compiled ModelicaModel (non-empty variables)
/// that haven't been wired yet (no SimComponent).
pub fn setup_balloon_wires(
    mut commands: Commands,
    q_new: Query<(Entity, &Name, &ModelicaModel), (With<BalloonModelMarker>, Without<SimComponent>)>,
) {
    for (entity, name, model) in &q_new {
        // Skip if compile hasn't produced outputs yet
        if model.variables.is_empty() {
            continue;
        }

        info!("Balloon '{name}' compiled — variables: {:?}", model.variables.keys().collect::<Vec<_>>());
        info!("Balloon '{name}' compiled — setting up SimComponent and wires");

        // Create SimComponent wrapping the ModelicaModel
        let comp = SimComponent {
            model_name: model.model_name.clone(),
            parameters: model.parameters.clone(),
            inputs: model.inputs.clone(),
            outputs: model.variables.clone(),
            status: if model.paused { SimStatus::Paused } else { SimStatus::Running },
            is_stepping: model.is_stepping,
        };
        commands.entity(entity).insert(comp);

        // BalloonVelocity tracks kinematic velocity — needed by apply_sim_forces
        commands.entity(entity).insert(BalloonVelocity(Vec3::ZERO));

        // Wire: balloon.netForce → avian.force_y (netForce already = buoyancy - weight - drag)
        commands.spawn(SimWire {
            start_element: entity, start_connector: "netForce".into(),
            end_element: entity, end_connector: "force_y".into(), scale: 1.0,
        });
        // Wire: balloon.volume → self.collider
        commands.spawn(SimWire {
            start_element: entity, start_connector: "volume".into(),
            end_element: entity, end_connector: "collider".into(), scale: 1.0,
        });
        // Wire: avian.height → balloon.height
        commands.spawn(SimWire {
            start_element: entity, start_connector: "height".into(),
            end_element: entity, end_connector: "height".into(), scale: 1.0,
        });
        // Wire: avian.velocity_y → balloon.velocity
        commands.spawn(SimWire {
            start_element: entity, start_connector: "velocity_y".into(),
            end_element: entity, end_connector: "velocity".into(), scale: 1.0,
        });

        info!("Balloon '{name}' — wires created");
        commands.entity(entity).remove::<BalloonModelMarker>();
    }
}

/// Syncs ModelicaModel.variables → SimComponent.outputs every frame.
///
/// Also copies inputs to outputs so that propagate_wires can read them
/// for self-loop wires (e.g., avian.height → balloon.height where the
/// wire reads from SimComponent.outputs to get the height value that was
/// written to SimComponent.inputs by the same propagate_wires system).
pub fn sync_modelica_outputs(
    mut q_models: Query<(&ModelicaModel, &mut SimComponent), Without<BalloonModelMarker>>,
) {
    for (model, mut comp) in &mut q_models {
        // Copy computed variables (outputs) from the Modelica worker: netForce,
        // volume, temperature, buoyancy, etc.
        // Do NOT copy model.inputs here — if input names (height, velocity) were
        // in comp.outputs, propagate_wires would read them instead of the actual
        // AvianSim.outputs, creating a self-loop disconnected from physics.
        for (name, val) in &model.variables {
            comp.outputs.insert(name.clone(), *val);
        }
        comp.status = if model.paused { SimStatus::Paused } else { SimStatus::Running };
    }
}

/// Syncs wire inputs → ModelicaModel.inputs (Avian position → height/velocity).
pub fn sync_inputs_to_modelica(
    mut q_models: Query<(&SimComponent, &mut ModelicaModel), Without<BalloonModelMarker>>,
) {
    for (comp, mut model) in &mut q_models {
        for (name, val) in &comp.inputs {
            model.inputs.insert(name.clone(), *val);
        }
    }
}
