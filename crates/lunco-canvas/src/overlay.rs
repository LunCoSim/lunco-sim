//! Floating screen-space UI rendered on top of the scene.
//!
//! An [`Overlay`] is a post-pass that gets the canvas's widget rect
//! and the (non-mutable) canvas state. Unlike a [`crate::layer::Layer`]
//! it draws in **screen coordinates** (pinned to the widget corners,
//! not to the viewport) and it's free to consume pointer events over
//! its own rect — the canvas's input router asks overlays first.
//!
//! B1 ships **zero** concrete overlays — only the trait. The slot
//! exists so the following features land as single-file additions:
//!
//! - `MinimapOverlay` — miniature of the scene, draggable to pan.
//! - `NavBarOverlay` — zoom slider, fit-all button, breadcrumb.
//! - `StatsOverlay` — node/edge count, active tool, debug FPS.
//! - `SearchOverlay` — Ctrl+P to jump to a node by name.
//! - `PropertyPopoverOverlay` — hover-to-preview values, Figma-style.
//!
//! Each of these is roughly one file, one impl of this trait,
//! ~100-200 LOC. Nothing in the canvas core changes to add any of
//! them.

use bevy_egui::egui;

use crate::scene::{Rect, Scene};
use crate::selection::Selection;
use crate::viewport::Viewport;

/// Canvas state visible to overlays — read-only on scene/selection
/// so overlays can't accidentally mutate authored state; mutable
/// on viewport so a minimap can pan by dragging.
pub struct OverlayCtx<'a> {
    pub scene: &'a Scene,
    pub selection: &'a Selection,
    pub viewport: &'a mut Viewport,
    /// The widget rect the canvas is painting into — overlays anchor
    /// themselves to corners of this, not the window.
    pub canvas_screen_rect: Rect,
}

/// Floating UI layer. See module docs.
pub trait Overlay: Send + Sync {
    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut OverlayCtx);
    fn name(&self) -> &'static str;
}
