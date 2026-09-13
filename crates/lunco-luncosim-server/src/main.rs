//! Headless LunCo luncosim server.
//!
//! The same simulation runtime as the `luncosim` GUI — through the
//! [`lunco_luncosim_core`][lunco_luncosim_core] library — but without the GUI
//! shell. It calls [`run_headless`][lunco_luncosim_core::run_headless], which is
//! windowless (no window/winit/egui; sim + physics + cosim + networking host,
//! driven by
//! `ScheduleRunnerPlugin`). Built `-p lunco-luncosim-server`, the GUI stack isn't
//! linked at all because the core package has no GUI feature to unify.
//!
//! `cargo run -p lunco-luncosim-server` starts the sim and the networking host.
//! The HTTP command API needs `-- --api [PORT]`: the `server` feature compiles
//! it in, but headless does NOT imply a listening port, and nothing warns when
//! it isn't there — a client just gets connection-refused.
//!
//!     cargo run -p lunco-luncosim-server -- --api 4101
fn main() -> lunco_luncosim_core::AppExit {
    lunco_luncosim_core::run_headless()
}
