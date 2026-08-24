//! The windowed LunCoSim GUI. All application logic lives in [`lunco_luncosim::run`]
//! so the headless `luncosim-server` binary shares the same composition root.
//! Built with the default `ui` feature.
fn main() -> lunco_luncosim::AppExit {
    // Velopack must see the original process before CLI dispatch. It handles
    // install/update hooks and applies a package that was downloaded during a
    // previous run. It does not perform the GitHub update check; that remains
    // an explicit native GUI operation in the Updates settings menu.
    #[cfg(all(feature = "ui", not(target_arch = "wasm32")))]
    velopack::VelopackApp::build().run();

    #[cfg(not(target_family = "wasm"))]
    if std::env::args().skip(1).any(|a| a == "test") {
        std::process::exit(lunco_luncosim::debug_scene::run() as i32);
    }

    // `luncosim rhai [...]` is a client mode: talk to an already-running
    // instance over its `--api` port instead of opening a second window.
    #[cfg(not(target_family = "wasm"))]
    if lunco_luncosim::rhai_repl::run_if_requested() {
        return lunco_luncosim::AppExit::Success;
    }

    // `luncosim --validate <path>…` is a one-shot pre-flight. It must run before
    // the app is built, so validation never opens a window or initializes GPU.
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
            // The app is intentionally not constructed for pre-flight, so its
            // scripting plugin cannot register the authored lint policies.
            // Use the same registration owner as the live runtime before the
            // shared ValidateAsset entry point is called.
            lunco_scripting::register_builtin_policies();
            std::process::exit(lunco_scene_commands::validate::run_cli(&paths));
        }
    }
    lunco_luncosim::run()
}
