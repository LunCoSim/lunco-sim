use bevy::prelude::*;

pub mod backend;
pub mod commands;
pub mod python;
#[cfg(not(target_arch = "wasm32"))]
pub mod repl;
pub mod doc;
pub mod source_asset;

use std::collections::HashMap;
use lunco_doc::{DocumentId, DocumentHost};
use doc::{ScriptDocument, ScriptedModel};
#[cfg(feature = "python")]
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

        if !app.is_plugin_added::<source_asset::PythonSourceAssetPlugin>() {
            app.add_plugins(source_asset::PythonSourceAssetPlugin);
        }

        app.init_resource::<ScriptRegistry>();
        
        #[cfg(not(target_arch = "wasm32"))]
        {
            let repl = repl::spawn_repl_thread();
            app.insert_resource(repl);
        }

        app.register_type::<ScriptedModel>()
           .register_type::<doc::ScriptLanguage>();

        let python_status = python::get_python_status();
        app.insert_resource(python_status);

        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Update, repl::process_repl_commands);
        app.add_systems(FixedUpdate, run_scripted_models);

        // Pluggable script backends — one per language, per cargo feature.
        // The matching `RunPython` command is `#[cfg]`-gated on the same
        // feature, so the language only appears on the API when its runtime
        // is actually compiled in (no "accepted but no-op" lie). Python is
        // the only backend today.
        #[allow(unused_mut)]
        let mut backends = backend::ScriptBackends::default();
        #[cfg(feature = "python")]
        backends.insert(doc::ScriptLanguage::Python, Box::new(backend::PythonBackend));
        app.insert_resource(backends);

        #[cfg(feature = "python")]
        commands::register_all_commands(app);
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
                    let c_str = match std::ffi::CString::new(doc.source.as_str()) {
                        Ok(c) => c,
                        Err(_) => {
                            error!("ScriptedModel: source contains a NUL byte; skipping");
                            return;
                        }
                    };
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

