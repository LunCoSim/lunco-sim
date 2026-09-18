//! Thin LunCoSim process shell.
//!
//! The headless runtime belongs to `lunco-luncosim-runtime`. The GPU-backed
//! application composition belongs to `lunco-luncosim-ui`; keeping that edge
//! out of this crate makes headless builds independent of the UI composition.

use lunco_luncosim_core::AppExit;

/// Run the headless runtime or the interactive application shell selected by
/// the build and command-line mode.
pub fn run() -> AppExit {
    #[cfg(not(feature = "ui"))]
    {
        lunco_luncosim_runtime::run_headless()
    }

    #[cfg(feature = "ui")]
    {
        let headless = std::env::args().any(|arg| arg == "--no-ui")
            || std::env::var("LUNCO_NO_UI").is_ok_and(|value| !value.is_empty() && value != "0");
        if headless {
            return lunco_luncosim_runtime::run_headless();
        }
        lunco_luncosim_ui::run_gui()
    }
}
