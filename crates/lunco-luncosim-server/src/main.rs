//! Headless LunCo luncosim server.
//!
//! The same simulation runtime as the `luncosim` GUI — through the
//! [`lunco_luncosim_runtime`][lunco_luncosim_runtime] library — but without the GUI
//! shell. It calls `lunco_luncosim_runtime::run_headless`, which is
//! windowless (no window/winit/egui; sim + physics + cosim,
//! driven by
//! `ScheduleRunnerPlugin`). Built `-p lunco-luncosim-server`, the GUI stack isn't
//! linked at all because the core package has no GUI feature to unify.
//!
//! Networking is explicit: `--host [PORT]` admits a WebTransport listener;
//! without a networking flag the simulation runs locally. The HTTP command API
//! independently requires `--api [PORT]`.
//!
//!     target/debug/luncosim-server --host 5888 --api 4101

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> lunco_luncosim_core::AppExit {
    lunco_luncosim_runtime::run_headless()
}
