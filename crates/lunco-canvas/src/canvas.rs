//! The top-level [`Canvas`] struct — stateful container that owns
//! the scene, viewport, selection, and the plugin slots (tools,
//! layers, overlays).
//!
//! In B1 the [`Canvas::ui`] method is a skeleton — it walks the
//! layer list and draws the scene, but does not yet route input. B2
//! wires real input handling (pan/zoom/select/drag/connect); B3+ adds
//! overlays and a richer tool set.
//!
//! # Stateful, not a free function
//!
//! Earlier drafts made the canvas a free `canvas_ui(ui, scene, ...)`
//! function so callers owned everything. That broke down as soon as
//! we wanted animation state (smooth zoom), in-flight drag state,
//! rubber-band rect, or tool memory — each of those needed somewhere
//! to live across frames. Making the canvas a struct gives those
//! features a home without the caller taking on canvas-specific
//! bookkeeping.

use std::sync::Arc;

use bevy_egui::egui;

use crate::event::SceneEvent;
use crate::layer::{EdgesLayer, GridLayer, Layer, NodesLayer, SelectionLayer, ToolPreviewLayer};
use crate::overlay::Overlay;
use crate::scene::{Pos, Rect, Scene};
use crate::selection::Selection;
use crate::tool::{DefaultTool, Tool};
use crate::viewport::Viewport;
use crate::visual::{DrawCtx, VisualRegistry};

/// The canvas. One per open view; panels that show the same scene
/// are free to have their own `Canvas` (independent viewport /
/// selection) or share one.
pub struct Canvas {
    pub scene: Scene,
    pub viewport: Viewport,
    pub selection: Selection,

    /// Active tool. Only one at a time. Swap to change modes (select
    /// tool → annotate tool). B1 ships [`DefaultTool`].
    pub tool: Box<dyn Tool>,

    /// Ordered render pipeline. See [`crate::layer`] module docs.
    pub layers: Vec<Box<dyn Layer>>,

    /// Floating UI on top of the scene. B1 empty; see
    /// [`crate::overlay`] for planned concrete overlays.
    pub overlays: Vec<Box<dyn Overlay>>,

    /// Visual registry — shared between the node/edge layers so
    /// reconfiguring visuals only requires touching one place.
    /// `Arc` because the same registry is cheap to clone into each
    /// layer that needs it.
    pub registry: Arc<VisualRegistry>,
}

impl Canvas {
    /// Build a canvas with the default layer pipeline and the
    /// given visual registry. The registry is moved into an Arc
    /// so callers don't have to wrap it themselves.
    pub fn new(registry: VisualRegistry) -> Self {
        let registry = Arc::new(registry);
        let layers: Vec<Box<dyn Layer>> = vec![
            Box::new(GridLayer::default()),
            Box::new(EdgesLayer::new(registry.clone())),
            Box::new(NodesLayer::new(registry.clone())),
            Box::new(SelectionLayer),
            Box::new(ToolPreviewLayer::default()),
        ];
        Self {
            scene: Scene::new(),
            viewport: Viewport::default(),
            selection: Selection::default(),
            tool: Box::new(DefaultTool),
            layers,
            overlays: Vec::new(),
            registry,
        }
    }

    /// Render the canvas into the given egui UI and return any
    /// scene events produced during the frame. In B1 nothing emits
    /// events (no input wiring yet) — the return value is always
    /// empty but the signature is stable so B2 can start emitting
    /// without a caller-side change.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Vec<SceneEvent> {
        let events: Vec<SceneEvent> = Vec::new();

        // Reserve the full available area for the canvas. egui 0.32's
        // `allocate_exact_size` returns `(Rect, Response)` — first is
        // the allocated rect, second is the interaction response.
        let (rect, _response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        let screen_rect = Rect::from_min_max(
            Pos::new(rect.min.x, rect.min.y),
            Pos::new(rect.max.x, rect.max.y),
        );

        // Tick the viewport so any in-flight smooth motion advances.
        // Use `ctx.input(|i| i.unstable_dt)` — the name's "unstable"
        // just means "exact frame delta", which is exactly what we
        // want for frame-rate-independent easing.
        let dt = ui.ctx().input(|i| i.unstable_dt);
        let moved = self.viewport.tick(dt);
        if moved {
            ui.ctx().request_repaint();
        }

        // Time for animated visuals — monotonic seconds since the
        // egui context was created. Consistent for all visuals in
        // the frame.
        let time = ui.ctx().input(|i| i.time);

        // Run each layer. We scope the &mut ui in a sub-block so
        // overlay rendering (which also needs &mut ui) can reuse it
        // without lifetime fighting.
        {
            let extras: Box<dyn std::any::Any> = Box::new(());
            for layer in &mut self.layers {
                let mut ctx = DrawCtx {
                    ui,
                    viewport: &self.viewport,
                    screen_rect,
                    time,
                    extras: &*extras,
                };
                layer.draw(&mut ctx, &self.scene, &self.selection);
            }
        }

        // Overlays render in screen space, on top of the scene.
        for overlay in &mut self.overlays {
            let mut ctx = crate::overlay::OverlayCtx {
                scene: &self.scene,
                selection: &self.selection,
                viewport: &mut self.viewport,
                canvas_screen_rect: screen_rect,
            };
            overlay.render(ui, &mut ctx);
        }

        events
    }
}
