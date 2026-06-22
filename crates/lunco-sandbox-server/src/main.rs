//! Headless LunCo sandbox server.
//!
//! The exact same application as the `sandbox` GUI bin — it calls the same
//! [`lunco_sandbox::run`] — but this crate is built without the `ui` feature,
//! so `run()` takes its headless path (no window/winit/egui; sim + physics +
//! cosim + networking host, driven by `ScheduleRunnerPlugin`). Run with
//! `cargo run -p lunco-sandbox-server` — headless by default, no flags.
fn main() {
    lunco_sandbox::run();
}
