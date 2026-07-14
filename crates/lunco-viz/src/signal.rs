//! Signal model — **the data types now live in [`lunco_signal`]** and are re-exported
//! here, so every existing `lunco_viz::SignalRegistry` / `SignalRef` / `ScalarHistory`
//! caller is unchanged.
//!
//! They moved because `lunco-viz` links `bevy_egui → bevy_render → wgpu`, while a ring
//! buffer of `f64`s is data, not rendering: the telemetry sampler must push into it from
//! a headless `--no-ui` run, which cannot link a GPU stack. See the `lunco-signal` crate
//! docs and `docs/architecture/render-decoupling.md`.
//!
//! What stayed here is the one genuinely render-bound thing — turning a signal into a
//! *colour* — and that now comes from the **theme**.

pub use lunco_signal::{
    ScalarHistory, ScalarSample, SignalMeta, SignalRef, SignalRegistry, SignalType,
    DEFAULT_CAPACITY,
};

use bevy_egui::egui;

/// Deterministic colour for a signal path, shared across every plot surface (panel
/// `Graphs`, `VizPanel`, in-canvas `PlotNodeVisual`, the inspector). Same `path` ⇒ same
/// colour everywhere; stable across sessions so a saved layout reopens with consistent
/// legend colours.
///
/// **The palette comes from the theme** ([`lunco_theme::PlotTokens`]), not from a
/// hardcoded table. It used to be a fixed 12-entry Tab10 list baked into this module,
/// which meant plot colours were the only colours in the app that ignored the active
/// theme — the same saturated blues on a light background as on a dark one, and no way
/// to re-theme them. Now they are palette-derived like every other colour.
///
/// Pass the theme in (`lunco_theme::active(ui.ctx())` at any egui call site).
pub fn color_for_signal(theme: &lunco_theme::Theme, path: &str) -> egui::Color32 {
    theme.plot.color_for_path(path)
}
