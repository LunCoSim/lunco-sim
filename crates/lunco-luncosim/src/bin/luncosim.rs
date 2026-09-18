//! The LunCoSim process entry point. GUI composition is delegated to
//! `lunco-luncosim-ui`; the headless `luncosim-server` uses the runtime
//! package directly.

#[cfg(not(target_arch = "wasm32"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> lunco_luncosim_core::AppExit {
    // The updater package owns the Velopack process hook. It must see the original
    // process before CLI dispatch. It does not perform the GitHub update check;
    // that remains an explicit native GUI operation in the Updates menu.
    #[cfg(all(feature = "ui", feature = "updates", not(target_arch = "wasm32")))]
    lunco_updater::initialize_velopack();

    #[cfg(not(target_family = "wasm"))]
    if std::env::args()
        .skip(1)
        .any(|a| a == "test" || a == "test-component")
    {
        std::process::exit(lunco_scene_runner::run() as i32);
    }

    // `luncosim rhai [...]` is a client mode: talk to an already-running
    // instance over its `--api` port instead of opening a second window.
    #[cfg(not(target_family = "wasm"))]
    if let Some(code) = lunco_rhai_repl::run_if_requested() {
        std::process::exit(code);
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
                eprintln!(
                    "--validate needs at least one path (.mo/.usda/.sysml/.kerml/.wgsl/.rhai)"
                );
                std::process::exit(2);
            }
            // The app is intentionally not constructed for pre-flight. Resolve
            // the same authored application policy manifest used at startup,
            // but keep its derived registry local to this one-shot command.
            let mut policy_registry =
                lunco_scripting_rhai_world::policy::ScriptedPolicyRegistry::default();
            let report = lunco_scripting_rhai_world::policy::load_application_policies(
                &mut policy_registry,
                None,
            );
            if let Some(error) = report.error {
                eprintln!("--validate cannot load application policies: {error}");
                std::process::exit(1);
            }
            if !report.failed.is_empty() {
                eprintln!(
                    "--validate cannot load application policies: {}",
                    report.failed.join("; ")
                );
                std::process::exit(1);
            }
            std::process::exit(lunco_scene_validation::validate::run_cli(&paths));
        }
    }
    lunco_luncosim::run()
}
