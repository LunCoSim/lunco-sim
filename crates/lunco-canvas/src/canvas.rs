//! The top-level [`Canvas`] struct — stateful container that owns
//! the scene, viewport, selection, and the plugin slots (tools,
//! layers, overlays).
//!
//! # Frame flow
//!
//! 1. Allocate the widget rect + capture the egui `Response`.
//! 2. Translate egui input (pointer, keys, wheel) into
//!    [`InputEvent`](crate::event::InputEvent)s, dispatch each to the
//!    active tool. Tool returns `Consumed` or `Passthrough`.
//! 3. Built-in navigation handles passthrough events — middle/right-
//!    drag for pan, scroll for zoom-at-pointer, `F` for fit-all.
//! 4. `viewport.tick(dt)` advances any smooth motion; if anything
//!    moves, request a repaint.
//! 5. Walk layers in order, each getting a fresh `DrawCtx`.
//! 6. Run overlays (screen-space UI).
//! 7. Return accumulated [`SceneEvent`](crate::event::SceneEvent)s
//!    to the caller.
//!
//! # Why navigation is built-in, not a tool
//!
//! Pan/zoom work identically in every domain. Putting them behind
//! the `Tool` trait would mean every custom tool reimplements them
//! (or explicitly delegates). Built-in navigation runs whenever the
//! active tool returns `Passthrough`, so any tool gets pan/zoom for
//! free without writing any code.

use std::sync::Arc;

use bevy_egui::egui::{self, PointerButton};

use crate::event::{InputEvent, Modifiers, MouseButton, SceneEvent};
use crate::layer::{EdgesLayer, GridLayer, Layer, NodesLayer, SelectionLayer, ToolPreviewLayer};
use crate::overlay::Overlay;
use crate::scene::{Pos, Rect, Scene};
use crate::selection::Selection;
use crate::tool::{CanvasOps, DefaultTool, Tool, ToolOutcome};
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
    /// tool → annotate tool).
    pub tool: Box<dyn Tool>,

    /// Ordered render pipeline. See [`crate::layer`] module docs.
    pub layers: Vec<Box<dyn Layer>>,

    /// Floating UI on top of the scene.
    pub overlays: Vec<Box<dyn Overlay>>,

    /// Visual registry — shared between the node/edge layers so
    /// reconfiguring visuals only requires touching one place.
    pub registry: Arc<VisualRegistry>,

    /// Last observed pointer position in screen space — needed by
    /// keyboard-triggered zooms (e.g. `Ctrl+=`) that want a sensible
    /// pivot when there's no live scroll event. `None` before the
    /// first mouse-move over the widget.
    last_pointer_screen: Option<Pos>,
}

impl Canvas {
    /// Build a canvas with the default layer pipeline and the
    /// given visual registry.
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
            tool: Box::new(DefaultTool::new()),
            layers,
            overlays: Vec::new(),
            registry,
            last_pointer_screen: None,
        }
    }

    /// Render + handle input. Returns all scene events produced
    /// during this frame.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Vec<SceneEvent> {
        let mut events: Vec<SceneEvent> = Vec::new();

        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        let screen_rect = Rect::from_min_max(
            Pos::new(rect.min.x, rect.min.y),
            Pos::new(rect.max.x, rect.max.y),
        );

        // ── Input translation ────────────────────────────────────
        let modifiers = ui.ctx().input(|i| Modifiers {
            shift: i.modifiers.shift,
            ctrl: i.modifiers.command || i.modifiers.ctrl,
            alt: i.modifiers.alt,
        });

        // Remember the pointer screen position for keyboard zoom
        // pivots. Uses hover_pos so it works even when no button
        // is held.
        if let Some(p) = response.hover_pos() {
            self.last_pointer_screen = Some(Pos::new(p.x, p.y));
        }

        // Gather input events for this frame. Several egui signals
        // can fire simultaneously on the same frame (click + double-
        // click + drag_started); translate each into its own
        // InputEvent and dispatch in a deterministic order.
        let mut input_events: Vec<InputEvent> = Vec::new();

        // Primary button press — comes through `drag_started` (egui
        // fires this on ANY press, not only multi-frame drags).
        if response.drag_started_by(PointerButton::Primary) {
            if let Some(p) = response.interact_pointer_pos() {
                let screen = Pos::new(p.x, p.y);
                let world = self.viewport.screen_to_world(screen, screen_rect);
                input_events.push(InputEvent::PointerDown {
                    button: MouseButton::Primary,
                    world,
                    screen,
                    modifiers,
                });
            }
        }

        // Primary move while held.
        if response.dragged_by(PointerButton::Primary) {
            if let Some(p) = response.interact_pointer_pos() {
                let screen = Pos::new(p.x, p.y);
                let world = self.viewport.screen_to_world(screen, screen_rect);
                input_events.push(InputEvent::PointerMove {
                    world,
                    screen,
                    modifiers,
                });
            }
        }

        // Primary release.
        if response.drag_stopped_by(PointerButton::Primary) {
            let p = response.interact_pointer_pos().or(response.hover_pos());
            if let Some(p) = p {
                let screen = Pos::new(p.x, p.y);
                let world = self.viewport.screen_to_world(screen, screen_rect);
                input_events.push(InputEvent::PointerUp {
                    button: MouseButton::Primary,
                    world,
                    screen,
                    modifiers,
                });
            }
        }

        // Middle-button OR right-button drag → pan. We read directly
        // from `ctx.input().pointer` rather than `response.dragged_by`
        // because the latter depends on the widget's `Sense`: with
        // `Sense::click_and_drag()` only primary drags are reported,
        // so middle/right panning would silently no-op. Raw input
        // works regardless.
        //
        // Also gate on hovered() so we don't pan when the pointer is
        // over a popup or another panel.
        if response.hovered() || response.contains_pointer() {
            // Middle only — right-button is reserved for context
            // menu (the `drag_started_by(Secondary)` path above fires
            // ContextMenuRequested on press; panning with right would
            // fire a menu + pan on every drag, which is confusing).
            let (panning, pan_delta) = ui.ctx().input(|i| {
                let middle = i.pointer.button_down(PointerButton::Middle);
                (middle, i.pointer.delta())
            });
            if panning && pan_delta != egui::Vec2::ZERO {
                self.viewport.pan_by_world(
                    -pan_delta.x / self.viewport.zoom,
                    -pan_delta.y / self.viewport.zoom,
                );
            }
        }

        // Secondary press (right-click). We don't track drag state
        // for it — a single Down is enough for context menu.
        if response.drag_started_by(PointerButton::Secondary) {
            if let Some(p) = response.interact_pointer_pos() {
                let screen = Pos::new(p.x, p.y);
                let world = self.viewport.screen_to_world(screen, screen_rect);
                input_events.push(InputEvent::PointerDown {
                    button: MouseButton::Secondary,
                    world,
                    screen,
                    modifiers,
                });
            }
        }

        if response.double_clicked() {
            if let Some(p) = response.interact_pointer_pos().or(response.hover_pos()) {
                let screen = Pos::new(p.x, p.y);
                let world = self.viewport.screen_to_world(screen, screen_rect);
                input_events.push(InputEvent::DoubleClick {
                    world,
                    screen,
                    modifiers,
                });
            }
        }

        // Keys. Gate on "no other widget wants keyboard input" (e.g.
        // a focused text field) so F doesn't jump the diagram when
        // the user is typing. `hovered()` is too strict — the panel
        // layout may already steal hover to the tab bar or gutter.
        let keyboard_free = !ui.ctx().wants_keyboard_input();
        if keyboard_free && (response.hovered() || response.contains_pointer()) {
            let key_names: &[(&'static str, egui::Key)] = &[
                ("Delete", egui::Key::Delete),
                ("Backspace", egui::Key::Backspace),
                ("Escape", egui::Key::Escape),
                ("F", egui::Key::F),
            ];
            for (name, key) in key_names {
                if ui.ctx().input(|i| i.key_pressed(*key)) {
                    input_events.push(InputEvent::Key {
                        name,
                        modifiers,
                    });
                }
            }
        }

        // Scroll → zoom. Gate on hovered OR contains_pointer so the
        // widget rect catches the event even when the pointer is
        // over a tool preview (which is still inside our rect).
        if response.hovered() || response.contains_pointer() {
            let scroll = ui.ctx().input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.0 {
                if let Some(screen) = self.last_pointer_screen {
                    let world = self.viewport.screen_to_world(screen, screen_rect);
                    input_events.push(InputEvent::Scroll {
                        delta_y: scroll,
                        screen,
                        modifiers,
                    });
                    let _ = world; // stash left for future tool use
                }
            }
        }

        // ── Event dispatch ────────────────────────────────────────
        for ev in &input_events {
            let outcome = {
                let mut ops = CanvasOps {
                    scene: &mut self.scene,
                    selection: &mut self.selection,
                    viewport: &mut self.viewport,
                    events: &mut events,
                };
                self.tool.handle(ev, &mut ops)
            };
            if outcome == ToolOutcome::Passthrough {
                self.handle_navigation(ev, screen_rect);
            }
        }

        // Tool tick — gives state-ful tools a chance to update even
        // on frames with no input events.
        let dt = ui.ctx().input(|i| i.unstable_dt);
        {
            let mut ops = CanvasOps {
                scene: &mut self.scene,
                selection: &mut self.selection,
                viewport: &mut self.viewport,
                events: &mut events,
            };
            self.tool.tick(&mut ops, dt);
        }

        // ── Viewport tick ────────────────────────────────────────
        if self.viewport.tick(dt) {
            ui.ctx().request_repaint();
        }

        // ── Rendering ─────────────────────────────────────────────
        let time = ui.ctx().input(|i| i.time);
        {
            // Preview state from the active tool, in an Any box so
            // `ToolPreviewLayer` can read it via `ctx.extras`.
            let preview = self.tool.preview();
            let extras: Box<dyn std::any::Any> =
                Box::new(preview.unwrap_or_else(|| {
                    crate::tool::ToolPreview::RubberBand(Rect::default())
                }));
            // Hack for now: ToolPreviewLayer is a no-op in B2. In
            // B3 we'll make it read the `extras` box properly; for
            // the moment any non-empty box works.
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

    /// Built-in navigation handlers — runs on events the tool
    /// passed through.
    fn handle_navigation(&mut self, ev: &InputEvent, screen_rect: Rect) {
        match ev {
            InputEvent::Scroll {
                delta_y,
                screen,
                modifiers,
            } => {
                // Ctrl+scroll explicitly zooms; plain scroll also
                // zooms on most CAD tools — we do the latter. If an
                // app wants scroll-to-pan, swap here.
                let _ = modifiers;
                let factor = 1.0 + *delta_y * self.viewport.config.scroll_zoom_gain;
                if factor > 0.0 && (factor - 1.0).abs() > f32::EPSILON {
                    self.viewport.zoom_at(*screen, factor, screen_rect);
                }
            }
            InputEvent::Key { name, .. } => {
                if *name == "F" {
                    if let Some(world_rect) = self.scene.bounds() {
                        let (c, z) = self
                            .viewport
                            .fit_values(world_rect, screen_rect, 30.0);
                        self.viewport.set_target(c, z);
                    }
                }
            }
            _ => {}
        }
    }
}
