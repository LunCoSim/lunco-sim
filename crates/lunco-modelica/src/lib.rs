//! Native Modelica simulation integration using the Rumoca platform.
//! 
//! This crate provides a Bevy plugin to execute Modelica models as asynchronous, 
//! high-fidelity "Virtual Plants" within the simulation loop. 
//! 
//! Follows Constitution Article XI: All heavy math (solving) is offloaded to 
//! a dedicated background worker thread because Rumoca steppers are !Send.

use bevy::prelude::*;
use std::collections::{HashMap, VecDeque};
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
        .register_type::<ModelicaInput>()
        .register_type::<ModelicaOutput>()
        .add_systems(Update, (
            spawn_modelica_requests,
            handle_modelica_responses,
        ));
    }
}

#[derive(Resource)]
pub struct ModelicaChannels {
    pub tx: Sender<ModelicaCommand>,
    rx: Receiver<ModelicaResult>,
}

pub enum ModelicaCommand {
    Step {
        entity: Entity,
        model_path: String,
        model_name: String,
        inputs: Vec<(String, f64)>,
        dt: f64,
    },
    Compile {
        entity: Entity,
        model_name: String,
        source: String,
    },
    Despawn {
        entity: Entity,
    }
}

pub struct ModelicaResult {
    pub entity: Entity,
    pub new_time: f64,
    pub outputs: Vec<(String, f64)>,
    pub error: Option<String>,
    pub is_new_model: bool,
}

/// The background worker that owns the !Send SimSteppers.
fn modelica_worker(rx: Receiver<ModelicaCommand>, tx: Sender<ModelicaResult>) {
    let mut steppers: HashMap<Entity, SimStepper> = HashMap::default();

    while let Ok(cmd) = rx.recv() {
        match cmd {
            ModelicaCommand::Compile { entity, model_name, source } => {
                info!("Compiling live Modelica source for {}...", model_name);
                let temp_path = format!(".cache/modelica_temp_{:?}.mo", entity);
                if let Err(e) = std::fs::write(&temp_path, &source) {
                    let _ = tx.send(ModelicaResult {
                        entity,
                        new_time: 0.0,
                        outputs: Vec::new(),
                        error: Some(format!("Failed to write temp file: {:?}", e)),
                        is_new_model: false,
                    });
                    continue;
                }

                match Compiler::new().model(&model_name).compile_file(&temp_path) {
                    Ok(comp_res) => {
                        match SimStepper::new(&comp_res.dae, StepperOptions::default()) {
                            Ok(stepper) => {
                                steppers.insert(entity, stepper);
                                info!("Hot-reload successful for {}", model_name);
                                let _ = tx.send(ModelicaResult {
                                    entity,
                                    new_time: 0.0,
                                    outputs: Vec::new(),
                                    error: None,
                                    is_new_model: true,
                                });
                            }
                            Err(e) => {
                                let _ = tx.send(ModelicaResult {
                                    entity,
                                    new_time: 0.0,
                                    outputs: Vec::new(),
                                    error: Some(format!("Stepper Error: {:?}", e)),
                                    is_new_model: false,
                                });
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(ModelicaResult {
                            entity,
                            new_time: 0.0,
                            outputs: Vec::new(),
                            error: Some(format!("Compiler Error: {:?}", e)),
                            is_new_model: false,
                        });
                    }
                }
            }
            ModelicaCommand::Step { entity, model_path, model_name, inputs, dt } => {
                // Ensure stepper exists
                if !steppers.contains_key(&entity) {
                    info!("Initializing Modelica stepper for {} ({})", model_name, model_path);
                    let res = Compiler::new()
                        .model(&model_name)
                        .compile_file(&model_path);
                    
                    match res {
                        Ok(comp_res) => {
                            match SimStepper::new(&comp_res.dae, StepperOptions::default()) {
                                Ok(stepper) => {
                                    steppers.insert(entity, stepper);
                                }
                                Err(e) => {
                                    error!("Failed to create stepper: {:?}", e);
                                    continue;
                                }
                            }
                        }
                        Err(e) => {
                            error!("Failed to compile model: {:?}", e);
                            continue;
                        }
                    }
                }

                let stepper = steppers.get_mut(&entity).unwrap();

                // Apply inputs
                for (name, val) in inputs {
                    let _ = stepper.set_input(&name, val);
                }

                // Step
                if let Err(e) = stepper.step(dt) {
                    error!("Modelica step failed for entity {:?}: {:?}", entity, e);
                    let _ = tx.send(ModelicaResult {
                        entity,
                        new_time: stepper.time(),
                        outputs: Vec::new(),
                        error: Some(format!("Step Error: {:?}", e)),
                        is_new_model: false,
                    });
                    continue;
                }

                // Collect outputs
                let mut outputs = Vec::new();
                for name in stepper.variable_names() {
                    if let Some(val) = stepper.get(name) {
                        if val.is_finite() {
                            outputs.push((name.clone(), val));
                        } else {
                            warn!("Non-finite value for {} in entity {:?}", name, entity);
                        }
                    }
                }

                let _ = tx.send(ModelicaResult {
                    entity,
                    new_time: stepper.time(),
                    outputs,
                    error: None,
                    is_new_model: false,
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
    /// Last real-world time this model was stepped.
    pub last_step_time: f64,
    /// If true, the simulation will not propagate.
    pub paused: bool,
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
    time: Res<Time>,
    mut q_models: Query<(Entity, &mut ModelicaModel, Option<&Children>)>,
    q_inputs: Query<&ModelicaInput>,
) {
    let current_real_time = time.elapsed_secs_f64();

    for (entity, mut model, children) in q_models.iter_mut() {
        if model.is_stepping || model.paused { continue; }

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

        // Calculate dt based on real time elapsed since last request
        let mut dt = if model.last_step_time == 0.0 || model.paused {
            0.016 // First step default or reset timer while paused
        } else {
            (current_real_time - model.last_step_time).max(0.001)
        };
        
        // Cap dt to prevent solver explosions (Article XI adherence)
        if dt > 0.1 {
            dt = 0.1;
        }
        
        if !model.paused {
            model.last_step_time = current_real_time;
        }

        model.is_stepping = true;
        let _ = channels.tx.send(ModelicaCommand::Step {
            entity,
            model_path: model.model_path.clone(),
            model_name: model.model_name.clone(),
            inputs,
            dt,
        });
    }
}

fn handle_modelica_responses(
    channels: Res<ModelicaChannels>,
    mut q_models: Query<&mut ModelicaModel>,
    mut q_outputs: Query<(&mut ModelicaOutput, Option<&ChildOf>)>,
    mut workbench_state: ResMut<ui::WorkbenchState>,
) {
    while let Ok(result) = channels.rx.try_recv() {
        if result.error.is_some() {
            workbench_state.compilation_error = result.error;
        } else {
            // Clear error if we got a successful result for the selected entity
            if workbench_state.selected_entity == Some(result.entity) {
                workbench_state.compilation_error = None;
            }
        }

        if result.is_new_model {
            // Reset everything for this entity on new model load
            workbench_state.history.clear();
            if let Ok(mut model) = q_models.get_mut(result.entity) {
                model.current_time = 0.0;
                model.last_step_time = 0.0;
            }
        }

        if let Ok(mut model) = q_models.get_mut(result.entity) {
            model.current_time = result.new_time;
            model.is_stepping = false;

            // Update workbench history if this is the selected entity
            if workbench_state.selected_entity == Some(result.entity) {
                let time = result.new_time;
                let max_history = workbench_state.max_history;
                for (name, val) in &result.outputs {
                    let history = workbench_state.history.entry(name.clone()).or_insert_with(|| VecDeque::with_capacity(max_history));
                    history.push_back([time, *val]);
                    if history.len() > max_history {
                        history.pop_front();
                    }
                }
            }

            for (name, val) in result.outputs {
                for (mut output, child_of) in q_outputs.iter_mut() {
                    let is_target = if let Some(child_of) = child_of {
                        child_of.parent() == result.entity && output.variable_name == name
                    } else {
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
