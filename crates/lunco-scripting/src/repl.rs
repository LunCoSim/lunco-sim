//! Interactive stdin REPL — type a snippet, it runs against the live sim.
//!
//! Default language is **rhai** (the pure-Rust, wasm-clean backend): each line
//! is evaluated with full World access via [`world_bridge::eval_with_world`], so
//! the bridge verbs (`cmd()`, `get()`, `find()`, `world_pos()`, …) work — this is
//! the "run a repl-like script" console into a running process. When rhai is
//! compiled out but python is in, it falls back to the CPython path.

use std::io::{self, BufRead};
#[cfg(all(feature = "python", not(feature = "rhai")))]
use std::ffi::CString;
use crossbeam_channel::{Receiver, unbounded};
use bevy::prelude::*;
#[cfg(all(feature = "python", not(feature = "rhai")))]
use pyo3::prelude::*;

#[derive(Resource)]
pub struct ReplResource {
    pub receiver: Receiver<String>,
}

// `disallowed_methods` bans `std::thread::spawn` because it panics on wasm. The
// whole `repl` module is `#[cfg(not(target_arch = "wasm32"))]` (see lib.rs) —
// this is a blocking stdin reader, which the web target has no equivalent of, so
// it cannot be reached there. `AsyncComputeTaskPool` is not a substitute: on wasm
// it runs on the main thread, so a blocking read loop would hang the page.
#[allow(clippy::disallowed_methods)]
pub fn spawn_repl_thread() -> ReplResource {
    let (tx, rx) = unbounded();
    // The default backend is rhai; only a python-only build prompts for Python.
    let lang = if cfg!(feature = "rhai") { "rhai" } else { "python" };
    std::thread::spawn(move || {
        let stdin = io::stdin();
        println!(">>> LunCo REPL Ready ({lang}) — snippets run against the live sim");
        for line in stdin.lock().lines() {
            if let Ok(cmd) = line {
                if !cmd.trim().is_empty() {
                    let _ = tx.send(cmd);
                }
            }
        }
    });
    ReplResource { receiver: rx }
}

/// Rhai REPL drain (exclusive: needs `&mut World` for the bridge verbs). Each
/// queued stdin line is evaluated host-trusted against the live World and its
/// captured stdout / return value printed back to the console.
#[cfg(feature = "rhai")]
pub fn drain_repl_rhai(world: &mut World) {
    let lines: Vec<String> = {
        let Some(repl) = world.get_resource::<ReplResource>() else {
            return;
        };
        let mut v = Vec::new();
        while let Ok(cmd) = repl.receiver.try_recv() {
            v.push(cmd);
        }
        v
    };
    for cmd in lines {
        match crate::world_bridge::eval_with_world(world, &cmd) {
            Ok(out) => {
                let out = out.trim_end();
                if out.is_empty() {
                    println!("<ok>");
                } else {
                    println!("{out}");
                }
            }
            Err(e) => eprintln!("rhai error: {e}"),
        }
    }
}

/// Python REPL drain (only when rhai is compiled out). Runs each line in the
/// embedded interpreter; no World bridge (that's the rhai path).
#[cfg(all(feature = "python", not(feature = "rhai")))]
pub fn process_repl_commands(
    repl: Res<ReplResource>,
    python_status: Res<crate::python::PythonStatus>,
) {
    while let Ok(cmd) = repl.receiver.try_recv() {
        info!("Executing REPL: {}", cmd);
        if *python_status != crate::python::PythonStatus::Available {
            error!("Python is not available. Cannot execute REPL command.");
            continue;
        }
        Python::with_gil(|py| {
            let c_str = match CString::new(cmd.as_str()) {
                Ok(c) => c,
                Err(_) => {
                    error!("REPL: command contains a NUL byte; rejected");
                    return;
                }
            };
            match py.run(&c_str, None, None) {
                Ok(_) => {}
                Err(e) => {
                    error!("Python Error: {}", e);
                }
            }
        });
    }
}
