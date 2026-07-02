//! The windowed GUI sandbox. Thin shim over [`lunco_sandbox::run`]; all logic
//! (and the global allocator) lives in the crate's library so the headless
//! `sandbox-server` bin shares exactly the same app. Built with the default
//! `ui` feature ⇒ GUI.
fn main() -> lunco_sandbox::AppExit {
    // `sandbox rhai [...]` is a client mode: talk to an already-running instance
    // over its `--api` port instead of opening a window. Falls through to the GUI
    // for a normal launch.
    if lunco_sandbox::rhai_repl::run_if_requested() {
        return lunco_sandbox::AppExit::Success;
    }
    lunco_sandbox::run()
}
