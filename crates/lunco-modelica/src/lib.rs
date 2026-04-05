//! Native Modelica simulation integration using the Rumoca platform.
//! 
//! This crate provides a Bevy plugin to execute Modelica models as asynchronous, 
//! high-fidelity "Virtual Plants" within the simulation loop. 
//! 
//! Follows Constitution Article XI: All heavy math (solving) is offloaded to 
//! a dedicated background worker thread because Rumoca steppers are !Send.

use bevy::prelude::*;
use std::collections::HashMap;
use rumoca::Compiler;
use rumoca_sim::with_diffsol::stepper::{SimStepper, StepperOptions};
use crossbeam_channel::{unbounded, Receiver, Sender};

pub mod ui;

/// Plugin that manages the lifecycle of Modelica simulations.
pub struct LunCoModelicaPlugin;

impl Plugin for LunCoModelicaPlugin {
    fn build(&self, app: &mut App) {
        let (cmd_tx, cmd_rx) = unbounded::<ModelicaCommand>();
        let (res_tx, res_rx) = unbounded::<ModelicaResult>();

        // Spawn the dedicated Modelica worker thread (since SimStepper is !Send)
        std::thread::spawn(move || {
            modelica_worker(cmd_rx, res_tx);
        });

        app.insert_resource(ModelicaChannels {
            tx: cmd_tx,
            rx: res_rx,
        })
        .register_type::<ModelicaModel>()
        .add_systems(Update, (
            spawn_modelica_requests,
            handle_modelica_responses,
        ));
    }
}

#[derive(Resource)]
struct ModelicaChannels {
    tx: Sender<ModelicaCommand>,
    rx: Receiver<ModelicaResult>,
}

enum ModelicaCommand {
    Step {
        entity: Entity,
        model_path: String,
        model_name: String,
        inputs: Vec<(String, f64)>,
        dt: f64,
    },
    Despawn {
        entity: Entity,
    }
}

struct ModelicaResult {
    entity: Entity,
    new_time: f64,
    outputs: Vec<(String, f64)>,
}

/// The background worker that owns the !Send SimSteppers.
fn modelica_worker(rx: Receiver<ModelicaCommand>, tx: Sender<ModelicaResult>) {
    let mut steppers: HashMap<Entity, SimStepper> = HashMap::default();

    while let Ok(cmd) = rx.recv() {
        match cmd {
            ModelicaCommand::Step { entity, model_path, model_name, inputs, dt } => {
                // Ensure stepper exists
                let stepper = steppers.entry(entity).or_insert_with(|| {
                    info!("Initializing Modelica stepper for {} ({})", model_name, model_path);
                    let comp_res = Compiler::new()
                        .model(&model_name)
                        .compile_file(&model_path)
                        .expect("Failed to compile model"); // TODO: Handle error better
                    
                    SimStepper::new(&comp_res.dae, StepperOptions::default())
                        .expect("Failed to create stepper")
                });

                // Apply inputs
                for (name, val) in inputs {
                    let _ = stepper.set_input(&name, val);
                }

                // Step
                if let Err(e) = stepper.step(dt) {
                    error!("Modelica step failed for entity {:?}: {:?}", entity, e);
                }

                // Collect outputs
                let mut outputs = Vec::new();
                for name in stepper.variable_names() {
                    if let Some(val) = stepper.get(name) {
                        outputs.push((name.clone(), val));
                    }
                }

                let _ = tx.send(ModelicaResult {
                    entity,
                    new_time: stepper.time(),
                    outputs,
                });
            }
            ModelicaCommand::Despawn { entity } => {
                steppers.remove(&entity);
            }
        }
    }
}

/// Component that attaches a Modelica model to an entity.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
pub struct ModelicaModel {
    pub model_path: String,
    pub model_name: String,
    pub current_time: f64,
    #[reflect(ignore)]
    pub is_stepping: bool,
}

/// Component for mapping an ECS value to a Modelica input variable.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
pub struct ModelicaInput {
    pub variable_name: String,
    pub value: f64,
}

/// Component for mapping a Modelica output variable to an ECS state.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
pub struct ModelicaOutput {
    pub variable_name: String,
    pub value: f64,
}

fn spawn_modelica_requests(
    channels: Res<ModelicaChannels>,
    mut q_models: Query<(Entity, &mut ModelicaModel, Option<&Children>)>,
    q_inputs: Query<&ModelicaInput>,
) {
    for (entity, mut model, children) in q_models.iter_mut() {
        if model.is_stepping { continue; }

        let mut inputs = Vec::new();
        if let Some(children) = children {
            for child in children.iter() {
                if let Ok(input) = q_inputs.get(child) {
                    inputs.push((input.variable_name.clone(), input.value));
                }
            }
        }
        if let Ok(input) = q_inputs.get(entity) {
            inputs.push((input.variable_name.clone(), input.value));
        }

        model.is_stepping = true;
        let _ = channels.tx.send(ModelicaCommand::Step {
            entity,
            model_path: model.model_path.clone(),
            model_name: model.model_name.clone(),
            inputs,
            dt: 0.016,
        });
    }
}

fn handle_modelica_responses(
    channels: Res<ModelicaChannels>,
    mut q_models: Query<&mut ModelicaModel>,
    mut q_outputs: Query<(&mut ModelicaOutput, Option<&ChildOf>)>,
) {
    while let Ok(result) = channels.rx.try_recv() {
        if let Ok(mut model) = q_models.get_mut(result.entity) {
            model.current_time = result.new_time;
            model.is_stepping = false;

            for (name, val) in result.outputs {
                for (mut output, child_of) in q_outputs.iter_mut() {
                    let is_target = if let Some(child_of) = child_of {
                        child_of.parent() == result.entity && output.variable_name == name
                    } else {
                        // TODO: Also check if output is on the entity itself
                        false 
                    };

                    if is_target {
                        output.value = val;
                    }
                }
            }
        }
    }
}
