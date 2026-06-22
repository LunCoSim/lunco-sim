//! The windowed GUI sandbox. Thin shim over [`lunco_sandbox::run`]; all logic
//! (and the global allocator) lives in the crate's library so the headless
//! `sandbox-server` bin shares exactly the same app. Built with the default
//! `ui` feature ⇒ GUI.
fn main() {
    lunco_sandbox::run();
}
