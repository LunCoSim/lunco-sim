//! Shared web-frontend boot library for LunCoSim wasm apps.
//!
//! Two halves of one concern — dismissing the HTML loading screen the
//! moment the app is actually interactive (not a frame sooner):
//!
//!   - **Browser side** — `web/lunco-boot.{js,css}` (shipped as static
//!     assets, copied into every `dist/<app>/` by `scripts/build_web.sh`).
//!     `lunco-boot.js` exports `boot({ init, wasmUrl, wasmSize, title })`:
//!     it injects the loader card, streams the wasm download with a
//!     progress bar, forces `Content-Type: application/wasm` for
//!     streaming compile, calls the bundle's `init()`, and surfaces
//!     errors. A per-app `index.html` is reduced to a ~15-line config
//!     call. `lunco-boot.css` carries the loader styles, the `#bevy`
//!     focus-ring fix, and the dark backdrop — themeable via the
//!     `--lc-accent` / `--lc-backdrop` CSS variables.
//!   - **Rust side** — [`WebReadyPlugin`] (this file). After Bevy paints
//!     its first egui frame it calls `window.__lc_app_ready()`, which
//!     fades the loader out. Hiding earlier (on `init()` resolve) would
//!     flash a blank canvas during the plugin-build gap.
//!
//! Any LunCoSim wasm binary calls `app.add_plugins(WebReadyPlugin)`. On
//! native the plugin adds nothing, so the same line compiles everywhere.

use bevy::prelude::*;

/// Signals the HTML loading screen once the app has painted, by calling
/// the page's `window.__lc_app_ready()` hook (defined in `lunco-boot.js`).
/// No-op on native targets.
pub struct WebReadyPlugin;

impl Plugin for WebReadyPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(target_arch = "wasm32")]
        app.add_systems(Update, signal_ready_once_painted);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = app;
    }
}

/// Wait two `Update` frames so the first egui frame has been queued and
/// (likely) painted, then call `window.__lc_app_ready()` exactly once.
/// Subsequent frames early-return in O(1).
#[cfg(target_arch = "wasm32")]
fn signal_ready_once_painted(mut frame: Local<u32>) {
    use wasm_bindgen::JsCast;
    *frame += 1;
    if *frame != 2 {
        return;
    }
    let Some(win) = web_sys::window() else { return };
    let Ok(fnval) = js_sys::Reflect::get(&win, &"__lc_app_ready".into()) else {
        return;
    };
    let Ok(func) = fnval.dyn_into::<js_sys::Function>() else {
        return;
    };
    let _ = func.call0(&win.into());
}
