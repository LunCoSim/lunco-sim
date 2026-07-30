//! Headless LunCo luncosim server.
//!
//! The exact same application as the `luncosim` GUI bin — same
//! [`lunco_luncosim`][lunco_luncosim] library — but it calls [`run_headless`]
//! [lunco_luncosim::run_headless], which forces the windowless path (no
//! window/winit/egui; sim + physics + cosim + networking host, driven by
//! `ScheduleRunnerPlugin`). Built `-p lunco-luncosim-server`, the GUI stack isn't
//! linked at all; forcing the mode (vs. inferring it from the absent `ui`
//! feature) also keeps it headless if a `--workspace` build unifies `ui` on.
//!
//! `cargo run -p lunco-luncosim-server` starts the sim and the networking host.
//! The HTTP command API needs `-- --api [PORT]`: the `server` feature compiles
//! it in, but headless does NOT imply a listening port, and nothing warns when
//! it isn't there — a client just gets connection-refused.
//!
//!     cargo run -p lunco-luncosim-server -- --api 4101
fn main() -> lunco_luncosim::AppExit {
    lunco_luncosim::run_headless()
}
