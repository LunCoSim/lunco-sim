use std::io::{self, BufRead};
use std::ffi::CString;
use crossbeam_channel::{Receiver, unbounded};
use bevy::prelude::*;
use pyo3::prelude::*;

#[derive(Resource)]
pub struct ReplResource {
    pub receiver: Receiver<String>,
}

pub fn spawn_repl_thread() -> ReplResource {
    let (tx, rx) = unbounded();
    std::thread::spawn(move || {
        let stdin = io::stdin();
        // Use a simple prompt for the user
        println!(">>> LunCo REPL Ready (Python)");
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

pub fn process_repl_commands(repl: Res<ReplResource>) {
    while let Ok(cmd) = repl.receiver.try_recv() {
        info!("Executing REPL: {}", cmd);
        Python::with_gil(|py| {
            let c_str = CString::new(cmd.as_str()).unwrap();
            match py.run(&c_str, None, None) {
                Ok(_) => {}
                Err(e) => {
                    error!("Python Error: {}", e);
                }
            }
        });
    }
}
