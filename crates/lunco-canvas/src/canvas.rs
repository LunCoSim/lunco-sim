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

    /// When `true`, the active tool must not mutate the scene —
    /// no drag-to-move, no drag-to-connect, no Delete. Pan / zoom /
    /// selection still work. Set per-frame by the embedding app
    /// (e.g. `canvas_diagram.rs` flips it based on
    /// `WorkbenchState.open_model.read_only`).
    pub read_only: bool,

    /// Optional drag-to-grid snap. When `Some`, the default tool
    /// quantises every in-flight drag translation to multiples of
    /// `step` world units. Applied *live* so the user visually sees
    /// the icon click into alignment as they drag — not just at
    /// commit. Set per-frame by the embedding app (typically wired
    /// to a Settings toggle).
    pub snap: Option<SnapSettings>,
}

/// Grid-snap configuration for drag operations. Expressed in the
/// canvas's world units (not screen pixels) so the visible grid step
/// stays the same Modelica-coord-system grid regardless of zoom.
#[derive(Debug, Clone, Copy)]
pub struct SnapSettings {
    /// Grid step in world units. Common choices in Modelica tools:
    /// 2 (fine), 5 (default), 10 (coarse) of the 200-unit standard
    /// diagram coordinate system.
    pub step: f32,
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
            read_only: false,
            snap: None,
        }
    }

    /// Render + handle input. Returns the canvas's egui `Response`
    /// together with scene events produced this frame. The caller
    /// can call `.context_menu(|ui| ...)` on the response to attach
    /// a right-click menu using egui's native popup machinery.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> (egui::Response, Vec<SceneEvent>) {
        let mut events: Vec<SceneEvent> = Vec::new();

        // `Sense::click_and_drag()` covers primary interactions;
        // `| Sense::click()` adds secondary-click detection so egui
        // routes right-click into the Response (and
        // `Response::context_menu` works on it).
        let (rect, response) = ui.allocate_exact_size(
            ui.available_size(),
            egui::Sense::click_and_drag() | egui::Sense::click(),
        );
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

        // Primary button press / move / release via **raw input**.
        // `response.drag_started_by(Primary)` only fires when the
        // press turns into a drag — plain clicks (press + release
        // without movement) never enter any drag-* path, so the
        // tool never sees the interaction. Reading raw
        // `button_pressed` / `button_down` / `button_released`
        // covers clicks and drags uniformly.
        if response.hovered() || response.contains_pointer() {
            let (primary_pressed, primary_down, primary_released, pointer) = ui.ctx().input(|i| {
                (
                    i.pointer.button_pressed(PointerButton::Primary),
                    i.pointer.button_down(PointerButton::Primary),
                    i.pointer.button_released(PointerButton::Primary),
                    i.pointer.hover_pos(),
                )
            });
            if primary_pressed {
                if let Some(p) = pointer {
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
            // Emit a PointerMove every frame the primary button is
            // held, so the tool's drag promotion + in-flight drag
            // update both fire without depending on egui's
            // `dragged_by` gate.
            if primary_down && !primary_pressed {
                if let Some(p) = pointer {
                    let screen = Pos::new(p.x, p.y);
                    let world = self.viewport.screen_to_world(screen, screen_rect);
                    input_events.push(InputEvent::PointerMove {
                        world,
                        screen,
                        modifiers,
                    });
                }
            }
            if primary_released {
                if let Some(p) = pointer.or_else(|| {
                    // On release frame hover_pos can be stale; fall
                    // back to the response's own latest interaction
                    // position if egui has it.
                    response.interact_pointer_pos()
                }) {
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
        }

        // ── Middle-drag pan (unchanged) ──
        // Dedicated middle-button pan is kept as an alternate for
        // users with three-button mice who prefer it.
        if response.hovered() || response.contains_pointer() {
            let (middle_down, delta) = ui
                .ctx()
                .input(|i| (i.pointer.button_down(PointerButton::Middle), i.pointer.delta()));
            if middle_down && delta != egui::Vec2::ZERO {
                self.viewport.pan_by_world(
                    -delta.x / self.viewport.zoom,
                    -delta.y / self.viewport.zoom,
                );
            }
        }

        // Right-drag → pan. Right-CLICK (no drag) is caller's job
        // via `Response::context_menu` on the returned Response —
        // egui's native context-menu machinery handles open/close,
        // submenu nesting, and escape-to-dismiss. We only pan here.
        //
        // The `pan_delta.length_sq() > 4.0` gate means: start panning
        // only after the pointer has moved a few pixels. If the user
        // right-clicks without moving, delta stays near zero every
        // frame, no pan happens, and egui's context_menu opens on
        // release. Clean separation without state.
        if response.hovered() || response.contains_pointer() {
            let (secondary_down, delta) = ui
                .ctx()
                .input(|i| (i.pointer.button_down(PointerButton::Secondary), i.pointer.delta()));
            if secondary_down && delta.length_sq() > 0.25 {
                self.viewport.pan_by_world(
                    -delta.x / self.viewport.zoom,
                    -delta.y / self.viewport.zoom,
                );
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
                    read_only: self.read_only,
                    snap: self.snap,
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
                read_only: self.read_only,
                snap: self.snap,
            };
            self.tool.tick(&mut ops, dt);
        }

        // ── Viewport tick ────────────────────────────────────────
        if self.viewport.tick(dt) {
            ui.ctx().request_repaint();
        }

        // ── Rendering ─────────────────────────────────────────────
        let time = ui.ctx().input(|i| i.time);
        // Restrict painting to the canvas's allocated rect for the
        // remainder of this `ui` scope. Without this, layers and
        // overlays paint via `ctx.ui.painter()` whose clip is the
        // parent ui's max_rect — so nodes near the top of the
        // canvas's coordinate space spill out and visually overlap
        // sibling widgets above (e.g. the model-view toolbar). The
        // clip is intersected with the existing one, so no
        // unintended side effects in tightly-laid-out hosts.
        ui.set_clip_rect(rect);
        {
            // Pass the active tool's preview (ghost edge during a
            // port drag, rubber-band rect during band-select) via
            // `ctx.extras` so `ToolPreviewLayer` can render it.
            // `Option<ToolPreview>` so the layer can distinguish
            // "no preview" from "zero-length preview".
            let preview: Option<crate::tool::ToolPreview> = self.tool.preview();
            let extras: Box<dyn std::any::Any> = Box::new(preview);
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

        (response, events)
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
