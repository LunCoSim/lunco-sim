//! The windowed GUI sandbox. Thin shim over [`lunco_sandbox::run`]; all logic
//! (and the global allocator) lives in the crate's library so the headless
//! `sandbox-server` bin shares exactly the same app. Built with the default
//! `ui` feature ⇒ GUI.
fn main() -> lunco_sandbox::AppExit {
    // `sandbox rhai [...]` is a client mode: talk to an already-running instance
    // over its `--api` port instead of opening a window. Falls through to the GUI
    // for a normal launch. Native-only — `rhai_repl` uses raw `std::net`, so the
    // module is `#[cfg(not(target_family = "wasm"))]`; gate the call to match, or
    // the wasm sandbox bin fails to compile (E0433).
    #[cfg(not(target_family = "wasm"))]
    if lunco_sandbox::rhai_repl::run_if_requested() {
        return lunco_sandbox::AppExit::Success;
    }
    // `sandbox --validate <path>…` is a one-shot pre-flight: parse-only asset
    // checks (`lunco_scene_commands::validate`), report to stdout, exit 0/1.
    // Like `--help`, it must run BEFORE the app is built — no window, no GPU,
    // no Bevy `App`. Native-only: the check reads the local filesystem.
    #[cfg(not(target_family = "wasm"))]
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if let Some(pos) = args.iter().position(|a| a == "--validate") {
            let paths: Vec<String> = args[pos + 1..]
                .iter()
                .take_while(|a| !a.starts_with("--"))
                .cloned()
                .collect();
            if paths.is_empty() {
                eprintln!("--validate needs at least one path (.mo/.usda/.wgsl/.rhai/.btxml/.xml)");
                std::process::exit(2);
            }
            std::process::exit(lunco_scene_commands::validate::run_cli(&paths));
        }
    }
    lunco_sandbox::run()
}
