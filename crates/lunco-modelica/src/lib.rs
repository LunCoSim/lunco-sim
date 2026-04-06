//! Native Modelica simulation integration using the Rumoca platform.
//! 
//! This crate provides a Bevy plugin to execute Modelica models as asynchronous, 
//! high-fidelity "Virtual Plants" within the simulation loop. 

use bevy::prelude::*;
use std::collections::{HashMap, VecDeque};
use rumoca::Compiler;
use rumoca_sim::with_diffsol::stepper::{SimStepper, StepperOptions};
use rumoca_session::{Session, SessionConfig};
use crossbeam_channel::{unbounded, Receiver, Sender};
use regex::Regex;

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
    /// Re-initialize the stepper with new parameter values
    UpdateParameters {
        entity: Entity,
        model_path: String,
        model_name: String,
        parameters: Vec<(String, f64)>,
    },
    Reset {
        entity: Entity,
    },
    Despawn {
        entity: Entity,
    }
}

pub struct ModelicaResult {
    pub entity: Entity,
    pub new_time: f64,
    pub outputs: Vec<(String, f64)>,
    /// All detected variables and their current values
    pub detected_symbols: Vec<(String, f64)>,
    pub error: Option<String>,
    pub log_message: Option<String>,
    pub is_new_model: bool,
    /// If true, history and selections should be preserved (just a tune)
    pub is_parameter_update: bool,
}

/// The background worker that owns the !Send SimSteppers.
fn modelica_worker(rx: Receiver<ModelicaCommand>, tx: Sender<ModelicaResult>) {
    let mut steppers: HashMap<Entity, SimStepper> = HashMap::default();

    while let Ok(cmd) = rx.recv() {
        match cmd {
            ModelicaCommand::Reset { entity } => {
                steppers.remove(&entity);
                info!("Resetting Modelica stepper for entity {:?}", entity);
            }
            ModelicaCommand::UpdateParameters { entity, model_path, model_name, parameters } => {
                let res = Compiler::new()
                    .model(&model_name)
                    .compile_file(&model_path);
                
                match res {
                    Ok(comp_res) => {
                        let mut opts = StepperOptions::default();
                        opts.atol = 1e-3;
                        opts.rtol = 1e-3;

                        match SimStepper::new(&comp_res.dae, opts) {
                            Ok(mut stepper) => {
                                for (name, val) in parameters {
                                    let _ = stepper.set_input(&name, val);
                                }
                                let mut symbols = Vec::new();
                                for name in stepper.variable_names() {
                                    if let Some(val) = stepper.get(&name) {
                                        symbols.push((name.clone(), val));
                                    }
                                }
                                steppers.insert(entity, stepper);
                                let _ = tx.send(ModelicaResult {
                                    entity,
                                    new_time: 0.0,
                                    outputs: Vec::new(),
                                    detected_symbols: symbols,
                                    error: None,
                                    log_message: Some("Parameters applied successfully.".to_string()),
                                    is_new_model: false,
                                    is_parameter_update: true,
                                });
                            }
                            Err(e) => {
                                let _ = tx.send(ModelicaResult {
                                    entity,
                                    new_time: 0.0,
                                    outputs: Vec::new(),
                                    detected_symbols: Vec::new(),
                                    error: Some(format!("Init Error: {:?}", e)),
                                    log_message: None,
                                    is_new_model: false,
                                    is_parameter_update: true,
                                });
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(ModelicaResult {
                            entity,
                            new_time: 0.0,
                            outputs: Vec::new(),
                            detected_symbols: Vec::new(),
                            error: Some(format!("Re-compile Error: {:?}", e)),
                            log_message: None,
                            is_new_model: false,
                            is_parameter_update: true,
                        });
                    }
                }
            }
            ModelicaCommand::Compile { entity, model_name, source } => {
                let temp_dir = format!(".cache/modelica/{:?}", entity);
                let _ = std::fs::create_dir_all(&temp_dir);
                let temp_path = format!("{}/model.mo", temp_dir);

                if let Err(e) = std::fs::write(&temp_path, &source) {
                    let _ = tx.send(ModelicaResult {
                        entity,
                        new_time: 0.0,
                        outputs: Vec::new(),
                        detected_symbols: Vec::new(),
                        error: Some(format!("IO Error: {:?}", e)),
                        log_message: None,
                        is_new_model: false,
                        is_parameter_update: false,
                    });
                    continue;
                }

                match Compiler::new().model(&model_name).compile_file(&temp_path) {
                    Ok(comp_res) => {
                        let mut opts = StepperOptions::default();
                        opts.atol = 1e-3;
                        opts.rtol = 1e-3;

                        match SimStepper::new(&comp_res.dae, opts) {
                            Ok(stepper) => {
                                let mut symbols = Vec::new();
                                for name in stepper.variable_names() {
                                    if let Some(val) = stepper.get(&name) {
                                        symbols.push((name.clone(), val));
                                    }
                                }

                                steppers.insert(entity, stepper);
                                let _ = tx.send(ModelicaResult {
                                    entity,
                                    new_time: 0.0,
                                    outputs: Vec::new(),
                                    detected_symbols: symbols,
                                    error: None,
                                    log_message: Some(format!("Model '{}' compiled and loaded.", model_name)),
                                    is_new_model: true,
                                    is_parameter_update: false,
                                });
                            }
                            Err(e) => {
                                let _ = tx.send(ModelicaResult {
                                    entity,
                                    new_time: 0.0,
                                    outputs: Vec::new(),
                                    detected_symbols: Vec::new(),
                                    error: Some(format!("Stepper Error: {:?}", e)),
                                    log_message: None,
                                    is_new_model: false,
                                    is_parameter_update: false,
                                });
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(ModelicaResult {
                            entity,
                            new_time: 0.0,
                            outputs: Vec::new(),
                            detected_symbols: Vec::new(),
                            error: Some(format!("Compiler Error: {:?}", e)),
                            log_message: None,
                            is_new_model: false,
                            is_parameter_update: false,
                        });
                    }
                }
            }
            ModelicaCommand::Step { entity, model_path, model_name, inputs, dt } => {
                if !steppers.contains_key(&entity) {
                    let res = Compiler::new().model(&model_name).compile_file(&model_path);
                    if let Ok(comp_res) = res {
                        let mut opts = StepperOptions::default();
                        opts.atol = 1e-3;
                        opts.rtol = 1e-3;
                        if let Ok(mut s) = SimStepper::new(&comp_res.dae, opts) {
                            for (name, val) in &inputs { let _ = s.set_input(name, *val); }
                            steppers.insert(entity, s);
                        }
                    }
                }

                if let Some(stepper) = steppers.get_mut(&entity) {
                    for (name, val) in inputs { let _ = stepper.set_input(&name, val); }

                    let capped_dt = dt.min(0.033); 
                    let sub_dt = capped_dt / 3.0;
                    
                    let mut step_err = None;
                    for _ in 0..3 {
                        if let Err(e) = stepper.step(sub_dt) {
                            step_err = Some(e);
                            break;
                        }
                    }

                    if let Some(e) = step_err {
                        let _ = tx.send(ModelicaResult {
                            entity,
                            new_time: stepper.time(),
                            outputs: Vec::new(),
                            detected_symbols: Vec::new(),
                            error: Some(format!("Solver Error: {:?}", e)),
                            log_message: None,
                            is_new_model: false,
                            is_parameter_update: false,
                        });
                        steppers.remove(&entity);
                        continue;
                    }

                    let mut outputs = Vec::new();
                    for name in stepper.variable_names() {
                        if let Some(val) = stepper.get(name) {
                            if val.is_finite() {
                                outputs.push((name.clone(), val));
                            }
                        }
                    }

                    let _ = tx.send(ModelicaResult {
                        entity,
                        new_time: stepper.time(),
                        outputs,
                        error: None,
                        log_message: None,
                        is_new_model: false,
                        detected_symbols: Vec::new(),
                        is_parameter_update: false,
                    });
                }
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
    pub last_step_time: f64,
    pub paused: bool,
    /// Schema of the model: detected variable names and their roles.
    pub parameters: HashMap<String, f64>,
    pub inputs: HashMap<String, f64>,
    #[reflect(ignore)]
    pub is_stepping: bool,
}

fn spawn_modelica_requests(
    channels: Res<ModelicaChannels>,
    time: Res<Time>,
    mut q_models: Query<(Entity, &mut ModelicaModel)>,
) {
    let current_real_time = time.elapsed_secs_f64();

    for (entity, mut model) in q_models.iter_mut() {
        if model.is_stepping || model.paused { continue; }

        let mut inputs = Vec::new();
        for (name, val) in &model.inputs {
            inputs.push((name.clone(), *val));
        }

        let mut dt = if model.last_step_time == 0.0 || model.paused { 0.016 } 
                     else { (current_real_time - model.last_step_time).max(0.001) };
        if dt > 0.1 { dt = 0.1; }
        if !model.paused { model.last_step_time = current_real_time; }

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
    time: Res<Time>,
    mut q_models: Query<&mut ModelicaModel>,
    mut workbench_state: ResMut<ui::WorkbenchState>,
) {
    while let Ok(result) = channels.rx.try_recv() {
        if let Some(msg) = result.log_message {
            workbench_state.logs.push_back(msg);
            if workbench_state.logs.len() > 100 { workbench_state.logs.pop_front(); }
        }

        if let Some(err) = &result.error {
            workbench_state.compilation_error = Some(err.clone());
            workbench_state.logs.push_back(format!("ERROR: {}", err));
        } else if workbench_state.selected_entity == Some(result.entity) {
            workbench_state.compilation_error = None;
        }

        if let Ok(mut model) = q_models.get_mut(result.entity) {
            if result.is_new_model {
                workbench_state.history.remove(&result.entity);
                if workbench_state.selected_entity == Some(result.entity) {
                    workbench_state.plotted_variables.clear();
                }
                model.current_time = 0.0;
                model.last_step_time = 0.0;
            }

            if result.is_new_model || result.is_parameter_update {
                if !result.detected_symbols.is_empty() {
                    model.parameters.clear();
                    model.inputs.clear();
                    for (name, val) in &result.detected_symbols {
                        if name.ends_with("_in") {
                            model.inputs.insert(name.clone(), *val);
                        } else {
                            model.parameters.insert(name.clone(), *val);
                        }
                    }
                }
            }

            model.current_time = result.new_time;
            model.last_step_time = time.elapsed_secs_f64();
            model.is_stepping = false;

            let time_val = result.new_time;
            let max_history = workbench_state.max_history;
            let entity_history = workbench_state.history.entry(result.entity).or_insert_with(HashMap::new);
            
            for (name, val) in &result.outputs {
                let history = entity_history.entry(name.clone()).or_insert_with(|| VecDeque::with_capacity(max_history));
                history.push_back([time_val, *val]);
                if history.len() > max_history { history.pop_front(); }
            }
        }
    }
}

/// Helper to extract the first model/class/block name from a Modelica source string.
pub fn extract_model_name(source: &str) -> Option<String> {
    let re = Regex::new(r"(?m)^\s*(model|class|block|package)\s+([a-zA-Z0-9_]+)").ok()?;
    re.captures(source).map(|cap| cap[2].to_string())
}

/// Deprecated components for backward compatibility
#[derive(Component, Reflect, Default)]
pub struct ModelicaInput { pub variable_name: String, pub value: f64 }
#[derive(Component, Reflect, Default)]
pub struct ModelicaOutput { pub variable_name: String, pub value: f64 }
