//! One-shot script-execution commands.
//!
//! `RunRhai` / `RunPython` are typed `#[Command]`s — discoverable on every
//! transport (HTTP API, MCP, scripts) like any other command. `RunRhai` is
//! always present (pure-Rust, wasm-clean). `RunPython` is `#[cfg]`-gated on the
//! `python` feature, so it only appears in the API schema when the runtime is
//! actually compiled in. This is the fix for the original gap: the old
//! `ExecuteScript` was always advertised but silently no-op'd when no scripting
//! plugin handled it.
//!
//! The handler returns `Result<Ack, String>`; the `#[on_command]` macro records
//! the outcome under the request id, so callers poll `QueryCommandResult` for
//! the script's stdout (in `Ack.assigned.stdout`) or its error message.
//!
//! Adding another language later = a new `#[cfg(feature = "…")]` command here +
//! a backend in `backend.rs` + one line in the registration list.

#[cfg(any(feature = "rhai", feature = "python"))]
use crate::{backend::ScriptBackends, doc::ScriptLanguage};
#[cfg(any(feature = "rhai", feature = "python"))]
use bevy::prelude::*;
use lunco_core::register_commands;
#[cfg(any(feature = "rhai", feature = "python"))]
use lunco_core::{on_command, Ack, Command, OpId};

#[cfg(feature = "rhai")]
#[Command(default)]
pub struct RunRhai {
    pub code: String,
}

#[cfg(feature = "rhai")]
#[on_command(RunRhai)]
fn on_run_rhai(_t: On<RunRhai>, backends: Res<ScriptBackends>) -> Result<Ack, String> {
    let backend = backends
        .get(ScriptLanguage::Rhai)
        .ok_or_else(|| "rhai backend not registered".to_string())?;
    let stdout = backend.eval(&cmd.code)?;
    let mut ack = Ack::new(OpId::new());
    ack.assigned = serde_json::json!({ "stdout": stdout });
    Ok(ack)
}

#[cfg(feature = "python")]
#[Command(default)]
pub struct RunPython {
    pub code: String,
}

#[cfg(feature = "python")]
#[on_command(RunPython)]
fn on_run_python(_t: On<RunPython>, backends: Res<ScriptBackends>) -> Result<Ack, String> {
    let backend = backends
        .get(ScriptLanguage::Python)
        .ok_or_else(|| "python backend not registered".to_string())?;
    let stdout = backend.eval(&cmd.code)?;
    let mut ack = Ack::new(OpId::new());
    ack.assigned = serde_json::json!({ "stdout": stdout });
    Ok(ack)
}

// Generates `register_all_commands` for the compiled-in script commands. One
// cfg-exclusive invocation per feature combo so exactly one
// `register_all_commands` is emitted (covers the script-free build too).
#[cfg(all(feature = "rhai", feature = "python"))]
register_commands!(on_run_rhai, on_run_python);
#[cfg(all(feature = "rhai", not(feature = "python")))]
register_commands!(on_run_rhai,);
#[cfg(all(not(feature = "rhai"), feature = "python"))]
register_commands!(on_run_python,);
#[cfg(all(not(feature = "rhai"), not(feature = "python")))]
register_commands!();
