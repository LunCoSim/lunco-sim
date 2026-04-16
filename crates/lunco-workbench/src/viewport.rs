//! `ViewportPanel` — transparent centre placeholder so 3D apps still
//! get the full `egui_dock` experience.
//!
//! ## How it works today
//!
//! 3D apps register `ViewportPanel` in their workspace's centre slot
//! and let Bevy's primary camera render the 3D world to the **full
//! window**. `ViewportPanel` itself draws nothing — it's a transparent
//! tab body inside `egui_dock`. Result:
//!
//! - The 3D scene is visible in the dock's centre region (because
//!   the tab body is transparent and Bevy's render is behind egui).
//! - Side panels around it are full `egui_dock` participants —
//!   draggable, tabbable, splittable.
//! - The user can drag a real panel into the centre to tab it with
//!   the viewport (covers the 3D in that tab) — their choice.
//!
//! ## Why the camera isn't shrunk to the panel rect
//!
//! The "obviously right" implementation — write the panel's rect into
//! `Camera::viewport` — does NOT work with `bevy_egui`. `bevy_egui`
//! ties its egui surface to the primary camera's viewport, so
//! shrinking the camera also shrinks egui, which then reports a
//! smaller surface, which makes the panel computation think the
//! viewport should be smaller still — runaway feedback collapses the
//! window to 1×1 in a few frames. Fixed by leaving the camera at full
//! window and letting the 3D scene be visible everywhere the dock
//! isn't covering it.
//!
//! ## Future: render-to-texture
//!
//! For "3D scene strictly bounded to the panel rect" (so e.g. a left
//! side panel can be opaque without hiding part of the 3D), the
//! proper fix is to render the 3D camera into an offscreen `Image`
//! and display that image as an `egui::Image` here. Tracked as
//! follow-up.

use bevy::prelude::*;
use bevy_egui::egui;

use crate::{Panel, PanelId, PanelSlot};

/// Stable id for [`ViewportPanel`]. Use this in `Workspace::apply` to
/// place the viewport in a slot without instantiating the panel.
pub const VIEWPORT_PANEL_ID: PanelId = PanelId("workbench::viewport");

/// Marker component for cameras whose viewport tracks the
/// [`ViewportPanel`]'s rect.
///
/// Tag exactly one (or more — they all get the same viewport) of your
/// app's `Camera3d` entities with this component. The panel updates
/// `Camera::viewport` on every frame it renders.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct WorkbenchViewportCamera;

/// Panel that sizes a Bevy camera's viewport to its own rect.
///
/// Default slot is [`PanelSlot::Center`] — the typical place for a
/// viewport in an IDE-style layout.
pub struct ViewportPanel;

impl Panel for ViewportPanel {
    fn id(&self) -> PanelId {
        VIEWPORT_PANEL_ID
    }

    fn title(&self) -> String {
        // Empty title — there's nothing useful to show in a tab header
        // for "the 3D viewport". egui_dock still draws the bar (we
        // can't hide it per-leaf in 0.18) but the content is blank.
        String::new()
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Center
    }

    fn closable(&self) -> bool {
        // If the user closes the viewport tab, the centre region
        // collapses and the side panels reflow oddly. Keep it docked.
        false
    }

    fn transparent_background(&self) -> bool {
        // Crucial — without this, egui_dock fills the tab body with
        // an opaque colour and covers the 3D scene Bevy rendered behind.
        true
    }

    fn render(&mut self, _ui: &mut egui::Ui, _world: &mut World) {
        // Intentionally empty. Bevy's primary camera renders the 3D
        // world to the full window behind egui; this panel just
        // reserves a transparent slot in the dock so the side panels
        // get the full egui_dock UX (drag / split / tab).
    }
}
