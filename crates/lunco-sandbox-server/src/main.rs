//! Headless LunCo sandbox server.
//!
//! The exact same application as the `sandbox` GUI bin — same
//! [`lunco_sandbox`][lunco_sandbox] library — but it calls [`run_headless`]
//! [lunco_sandbox::run_headless], which forces the windowless path (no
//! window/winit/egui; sim + physics + cosim + networking host, driven by
//! `ScheduleRunnerPlugin`). Built `-p lunco-sandbox-server`, the GUI stack isn't
//! linked at all; forcing the mode (vs. inferring it from the absent `ui`
//! feature) also keeps it headless if a `--workspace` build unifies `ui` on.
//! Run with `cargo run -p lunco-sandbox-server` — headless, no flags.
fn main() {
    lunco_sandbox::run_headless();
}
