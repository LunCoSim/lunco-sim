//! Pluggable script-execution backends.
//!
//! One backend per language, registered in [`ScriptBackends`] by
//! `LunCoScriptingPlugin` under the matching cargo feature. The one-shot
//! command handler (`RunPython`) dispatches through this registry instead of
//! hard-coding an interpreter — so adding a language later (e.g. Lua) is "add
//! a feature + a backend + a command", not "edit every call site". Python is
//! the only backend today.

use crate::doc::ScriptLanguage;
use bevy::prelude::*;
use std::collections::HashMap;

/// A language runtime that can evaluate a one-shot snippet and return its
/// captured stdout (or an error message).
pub trait ScriptBackend: Send + Sync {
    fn eval(&self, code: &str) -> Result<String, String>;
}

/// Registry of available script backends, keyed by language. Populated
/// per-feature at plugin build; a language with no backend has no command
/// (the `#[Command]` is `#[cfg]`-gated on the same feature).
#[derive(Resource, Default)]
pub struct ScriptBackends {
    map: HashMap<ScriptLanguage, Box<dyn ScriptBackend>>,
}

impl ScriptBackends {
    pub fn insert(&mut self, lang: ScriptLanguage, backend: Box<dyn ScriptBackend>) {
        self.map.insert(lang, backend);
    }
    pub fn get(&self, lang: ScriptLanguage) -> Option<&dyn ScriptBackend> {
        self.map.get(&lang).map(|b| b.as_ref())
    }
}

/// Pure-Rust backend (rhai). The default, browser-capable runtime: compiles to
/// `wasm32-unknown-unknown`, sandboxed (op/depth/size caps), deterministic.
/// Gated on the `rhai` feature (on by default).
#[cfg(feature = "rhai")]
pub struct RhaiBackend;

#[cfg(feature = "rhai")]
impl ScriptBackend for RhaiBackend {
    fn eval(&self, code: &str) -> Result<String, String> {
        use std::sync::{Arc, Mutex};

        let mut engine = rhai::Engine::new();

        // Sandbox caps — defend against runaway / oversized untrusted scripts.
        engine.set_max_operations(1_000_000);
        engine.set_max_call_levels(64);
        engine.set_max_string_size(64 * 1024);
        engine.set_max_array_size(10_000);

        // Capture `print(...)` output so callers get script stdout, mirroring
        // the Python backend's StringIO redirect.
        let out = Arc::new(Mutex::new(String::new()));
        let sink = out.clone();
        engine.on_print(move |s| {
            if let Ok(mut buf) = sink.lock() {
                buf.push_str(s);
                buf.push('\n');
            }
        });

        let result = engine
            .eval::<rhai::Dynamic>(code)
            .map_err(|e| e.to_string())?;

        let mut captured = out
            .lock()
            .map_err(|_| "print buffer poisoned".to_string())?
            .clone();
        if !result.is_unit() {
            captured.push_str(&result.to_string());
        }
        Ok(captured)
    }
}

/// CPython backend (via pyo3). Captures stdout so callers get script output.
#[cfg(feature = "python")]
pub struct PythonBackend;

#[cfg(feature = "python")]
impl ScriptBackend for PythonBackend {
    fn eval(&self, code: &str) -> Result<String, String> {
        use pyo3::prelude::*;
        use pyo3::types::PyAnyMethods;

        if crate::python::get_python_status() != crate::python::PythonStatus::Available {
            return Err("Python is not available on this system".to_string());
        }
        let c_str =
            std::ffi::CString::new(code).map_err(|_| "code contains a NUL byte".to_string())?;

        Python::with_gil(|py| {
            // Redirect sys.stdout to an io.StringIO so the snippet's prints
            // are captured and returned to the caller.
            let sys = py.import("sys").map_err(|e| e.to_string())?;
            let io = py.import("io").map_err(|e| e.to_string())?;
            let buf = io
                .getattr("StringIO")
                .map_err(|e| e.to_string())?
                .call0()
                .map_err(|e| e.to_string())?;
            let prev = sys.getattr("stdout").map_err(|e| e.to_string())?;
            sys.setattr("stdout", &buf).map_err(|e| e.to_string())?;

            let run_result = py.run(&c_str, None, None);

            // Restore stdout before propagating, then read what was captured.
            let _ = sys.setattr("stdout", prev);
            let out: String = buf
                .getattr("getvalue")
                .map_err(|e| e.to_string())?
                .call0()
                .map_err(|e| e.to_string())?
                .extract()
                .map_err(|e| e.to_string())?;

            run_result.map_err(|e| e.to_string())?;
            Ok(out)
        })
    }
}
