use bevy::prelude::*;

pub mod python;
pub mod repl;
pub mod doc;

use std::collections::HashMap;
use lunco_doc::{DocumentId, DocumentHost};
use doc::{ScriptDocument, ScriptedModel};
use pyo3::types::{PyDictMethods, PyAnyMethods};

#[derive(Resource, Default)]
pub struct ScriptRegistry {
    pub documents: HashMap<DocumentId, DocumentHost<ScriptDocument>>,
}

pub struct LunCoScriptingPlugin;

impl Plugin for LunCoScriptingPlugin {
    fn build(&self, app: &mut App) {
        info!("Initializing LunCo Scripting Bridge...");
        python::initialize_python();
        
        app.init_resource::<ScriptRegistry>();
        
        let repl = repl::spawn_repl_thread();
        app.insert_resource(repl);
        
        app.register_type::<ScriptedModel>()
           .register_type::<doc::ScriptLanguage>();

        let python_status = python::get_python_status();
        app.insert_resource(python_status);

        app.add_systems(Update, repl::process_repl_commands);
        app.add_systems(FixedUpdate, run_scripted_models);

        // Handle remote script requests from the API
        app.add_observer(handle_script_request);
    }
}

fn run_scripted_models(
    mut q_models: Query<&mut ScriptedModel>,
    registry: Res<ScriptRegistry>,
    python_status: Res<python::PythonStatus>,
) {
    for mut model in q_models.iter_mut() {
        if model.paused { continue; }
        
        let Some(doc_id_raw) = model.document_id else { continue };
        let doc_id = DocumentId::new(doc_id_raw);
        let Some(host) = registry.documents.get(&doc_id) else { continue };
        let doc = host.document();

        // Execution logic for Python/Lua
        if doc.language == doc::ScriptLanguage::Python {
            #[cfg(feature = "python")]
            {
                if *python_status != python::PythonStatus::Available {
                    error_once!("Python is not available on this system. Cannot run Python scripts.");
                    continue;
                }
                pyo3::Python::with_gil(|py| {
                    // 1. Prepare inputs
                    let locals = pyo3::types::PyDict::new(py);
                    let inputs_dict = pyo3::types::PyDict::new(py);
                    for (k, v) in &model.inputs {
                        let _ = inputs_dict.set_item(k, v);
                    }
                    let outputs_dict = pyo3::types::PyDict::new(py);
                    for (k, v) in &model.outputs {
                        let _ = outputs_dict.set_item(k, v);
                    }
                    let _ = locals.set_item("inputs", inputs_dict);
                    let _ = locals.set_item("outputs", outputs_dict);

                    // 2. Run source
                    let c_str = std::ffi::CString::new(doc.source.as_str()).unwrap();
                    if let Err(e) = py.run(&c_str, None, Some(&locals)) {
                        error!("ScriptedModel Python Error: {}", e);
                    } else {
                        // 3. Extract outputs
                        if let Ok(Some(outputs)) = locals.get_item("outputs") {
                            if let Ok(dict) = outputs.downcast::<pyo3::types::PyDict>() {
                                for (k, v) in dict.iter() {
                                    if let (Ok(key), Ok(val)) = (k.extract::<String>(), v.extract::<f64>()) {
                                        model.outputs.insert(key, val);
                                    }
                                }
                            }
                        }
                    }
                });
            }
            #[cfg(not(feature = "python"))]
            {
                error_once!("Python support was not compiled into this binary.");
            }
        }
    }
}

fn handle_script_request(
    trigger: On<lunco_api::ScriptRequestEvent>,
    python_status: Res<python::PythonStatus>,
) {
    let event = trigger.event();
    info!("Executing Remote Script ({}): {}", event.language, event.code);
    
    if event.language.to_lowercase() == "python" {
        #[cfg(feature = "python")]
        {
            if *python_status != python::PythonStatus::Available {
                error!("Python is not available on this system. Cannot execute remote Python script.");
                return;
            }
            pyo3::Python::with_gil(|py| {
                let c_str = std::ffi::CString::new(event.code.as_str()).unwrap();
                if let Err(e) = py.run(&c_str, None, None) {
                    error!("Remote Python Error: {}", e);
                }
            });
        }
        #[cfg(not(feature = "python"))]
        {
            error!("Python support was not compiled into this binary.");
        }
    } else {
        warn!("Unsupported script language: {}", event.language);
    }
}
