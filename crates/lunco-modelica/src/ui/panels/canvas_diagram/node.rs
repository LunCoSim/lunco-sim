//! Node visual + per-component painting helpers for the canvas
//! diagram.
//!
//! Houses [`IconNodeData`] (typed payload carried in
//! `lunco_canvas::Node.data` for `"modelica.icon"` nodes),
//! [`IconNodeVisual`] (the per-component `NodeVisual` that paints the
//! icon body, ports, label and per-instance widgets), and the leaf
//! paint helpers [`paint_hover_card`] and [`paint_flow_dots`].

use bevy_egui::egui;
use lunco_canvas::{DrawCtx, Node as CanvasNode, NodeVisual, Pos as CanvasPos};
use lunco_theme::ColorAlpha;

use super::edge::port_edge_dir;
use super::paint::paint_dashed_rect;
use super::port::{
    PortShape, paint_input_control_widget, paint_port_shape,
};
use super::theme::{canvas_theme_from_ctx, modelica_icon_palette_from_ctx};

/// Typed payload carried in `lunco_canvas::Node.data` for every
/// `"modelica.icon"` node. Replaces the prior `serde_json::Value`
/// round-trip — projector boxes one of these, the visual factory
/// downcasts at construction. The Modelica primitive types it
/// carries (`Icon`, parameters) all derive `Serialize`/`Deserialize`,
/// so a future Scene snapshot story can serialize this struct
/// directly via a per-domain registry.
#[derive(Clone, Debug, Default)]
pub struct IconNodeData {
    /// Fully-qualified type name (e.g. `Modelica.Electrical.Analog.Basic.Resistor`).
    pub qualified_type: String,
    /// `Icons.*` package class — rendered with a dashed border so
    /// users see at a glance the component is decorative.
    pub icon_only: bool,
    /// `expandable connector` (MLS §9.1.3) — accent dashed border.
    pub expandable_connector: bool,
    /// Decoded `Icon(graphics={...})` annotation merged across the
    /// `extends` chain. `None` only when the class has literally no
    /// Icon in inheritance — then the visual falls back to a label box.
    pub icon_graphics: Option<crate::annotations::Icon>,
    /// Decoded `Diagram(graphics={...})` annotation, populated only
    /// for connector classes that author one. When set the renderer
    /// uses this instead of `icon_graphics` — MSL signal connectors
    /// (RealInput, RealOutput, …) put the `%name` text label and
    /// the larger filled triangle in their Diagram annotation, while
    /// keeping a stripped-down Icon for use as a port marker.
    pub diagram_graphics: Option<crate::annotations::Diagram>,
    /// Per-instance rotation (degrees CCW, Modelica convention).
    pub rotation_deg: f32,
    /// Mirror flags applied before rotation (MLS Annex D order).
    pub mirror_x: bool,
    pub mirror_y: bool,
    /// Instance name — drives `%name` text substitution.
    pub instance_name: String,
    /// Pre-formatted `(param_name, value)` for `%paramName` text
    /// substitution. Class defaults today; instance modifications
    /// follow.
    pub parameters: Vec<(String, String)>,
    /// Per-port connector-icon descriptors: `(port_name,
    /// connector_class_qualified_path, size_x, size_y, rotation_deg)`.
    /// The painter renders each connector class's authored `Icon` at
    /// the port location, sized + rotated per the port's authored
    /// `Placement(transformation(extent=..., rotation=...))`. Empty
    /// path falls back to the generic per-shape marker.
    pub port_connector_paths: Vec<(String, String, f32, f32, f32)>,
    /// Pre-resolved `Icon` for each port's connector class, indexed
    /// parallel to `port_connector_paths`. Resolved off-thread in
    /// `project_scene` so the painter never holds the engine lock —
    /// inline resolution previously locked 30+ times per frame on
    /// PID-class diagrams. `None` when the connector class has no
    /// Icon in its inheritance chain or hasn't been indexed yet.
    pub port_connector_icons: Vec<Option<crate::annotations::Icon>>,
    /// Conditional component (`Component X if <cond>`). Renderer
    /// halves opacity so users can see it's design-time visible but
    /// runtime-conditional — matches OMEdit/Dymola convention.
    pub is_conditional: bool,
}

/// Per-component icon visual. Renders, in priority order:
///
/// 1. The class's decoded `Icon(graphics={...})` annotation merged
///    across the `extends` chain — the only icon source. Painted via
///    [`crate::icon_paint::paint_graphics`] with lyon-tessellated
///    fills (EvenOdd, matching OMEdit/Dymola).
/// 2. A stylised rounded-rectangle fallback with the type label, used
///    only when the class has no `Icon` annotation anywhere in its
///    inheritance chain.
///
/// Ports render as filled dots on the icon boundary in all cases.
#[derive(Default)]
pub(super) struct IconNodeVisual {
    /// Type name ("Resistor", "Capacitor"…) shown under the instance
    /// label when the class has no Icon at all.
    pub(super) type_label: String,
    /// Pure-icon class (zero connectors, `.Icons.*` subpackage).
    /// Rendered with a dashed border so users can tell at a glance
    /// the component is decorative. Set by the projector via the
    /// node's `data.icon_only` flag.
    pub(super) icon_only: bool,
    /// `expandable connector` class (MLS §9.1.3). Rendered with a
    /// dashed border in an accent colour so users can distinguish
    /// them from regular connectors — expandable connectors collect
    /// variables across connections dynamically and have different
    /// semantics.
    pub(super) expandable_connector: bool,
    /// Decoded graphics from the class's `Icon` annotation. When
    /// present, takes precedence over the SVG icon path so user
    /// classes show their authored graphics instead of falling back
    /// to a generic placeholder.
    pub(super) icon_graphics: Option<crate::annotations::Icon>,
    /// Conditional component flag — render dimmed.
    pub(super) is_conditional: bool,
    /// Pre-formatted `(parameter_name, value)` pairs for `%paramName`
    /// text substitution. Carries class defaults from
    /// `crate::index::ClassEntry.parameters` (instance-modification overlay
    /// is a follow-up — most icons display defaults anyway when no
    /// instance modifications are set).
    pub(super) parameters: Vec<(String, String)>,
    /// Per-instance rotation (degrees CCW, Modelica frame) applied to
    /// the icon body itself — rotates both the SVG raster and the
    /// `paint_graphics` primitives uniformly. Without this, mirror /
    /// rotated MSL placements showed correct port positions but a
    /// wrong-looking body.
    pub(super) rotation_deg: f32,
    /// Mirror flags applied to the icon body, before rotation
    /// (MLS Annex D).
    pub(super) mirror_x: bool,
    pub(super) mirror_y: bool,
    /// Instance name this component is drawn for — "R1", "C1", …
    /// Drives the `%name` substitution in authored `Text` primitives
    /// (Modelica's convention for showing the instance label on the
    /// icon body). Empty when the projector didn't provide one.
    pub(super) instance_name: String,
    /// Class name (leaf — e.g. "Resistor"). Drives `%class`
    /// substitution in authored `Text` primitives.
    pub(super) class_name: String,
    /// `(port_name, connector_class_qualified_path, size_x, size_y,
    /// rotation_deg)` from the projected scene.
    pub(super) port_connector_paths: Vec<(String, String, f32, f32, f32)>,
    /// Pre-resolved connector-class `Icon` for each port, indexed
    /// parallel to `port_connector_paths`. See [`IconNodeData`] for
    /// the rationale (off-thread resolution to keep paint lock-free).
    pub(super) port_connector_icons: Vec<Option<crate::annotations::Icon>>,
    /// Parent component's fully-qualified type — used as the scope
    /// root when the indexer wrote a short connector path like
    /// `"RealInput"` and we need to resolve it via package walk.
    pub(super) parent_qualified_type: String,
}

impl NodeVisual for IconNodeVisual {
    fn draw(&self, ctx: &mut DrawCtx, node: &CanvasNode, selected: bool) {
        let r = ctx
            .viewport
            .world_rect_to_screen(node.rect, ctx.screen_rect);
        let rect = egui::Rect::from_min_max(
            egui::pos2(r.min.x, r.min.y),
            egui::pos2(r.max.x, r.max.y),
        );
        // Hard-clip every primitive this draw emits to the canvas's
        // allocated rect. `Canvas::ui` already calls
        // `ui.set_clip_rect(rect)`, but in some host layouts (docked
        // canvas next to a side panel) that implicit clip wasn't
        // holding — icons authored to extend past their node rect
        // (Modelica labels, port arrows on a node near the right
        // edge) bled into the neighbouring telemetry / inspector
        // panes. Re-applying the clip on the painter we hand to
        // `paint_graphics_themed` / `paint_port_shape` makes the
        // boundary explicit at the paint site, the same way
        // `paint_hover_card` already does for the foreground tooltip
        // layer.
        // Use the ui's *current* clip (already intersected with the
        // host pane in `Canvas::ui`), not `ctx.screen_rect`.
        // `screen_rect` is the canvas's full allocated rect, which
        // can extend past the visible pane when the host (egui_dock
        // leaf, scroll viewport) is narrower than the allocation —
        // clipping to it would still let icons paint over the
        // neighbour pane.
        let canvas_clip = ctx.ui.clip_rect();
        let clipped_painter = ctx.ui.painter().clone().with_clip_rect(canvas_clip);
        let theme_snap = canvas_theme_from_ctx(ctx.ui.ctx());
        // Conditional components (`Component X if cond`) — render at
        // reduced opacity so every primitive (icon shapes, text,
        // port markers) inherits the dimming. Matches OMEdit/Dymola
        // convention for "design-time visible, runtime-conditional"
        // components.
        let _dimmed_painter;
        let painter: &egui::Painter = if self.is_conditional {
            let mut p = clipped_painter.clone();
            p.set_opacity(0.4);
            _dimmed_painter = p;
            &_dimmed_painter
        } else {
            &clipped_painter
        };

        // No always-on card fill. Icons that need a body (Resistor's
        // white rectangle, Inertia's gray cylinder, …) author it
        // themselves; classes without an Icon at all get the
        // placeholder card from the `!drew_icon` branch below.
        // Matches Dymola/OMEdit — they never paint a "competing"
        // card behind authored icons.

        // Authored graphics from the class's `Icon` annotation,
        // merged across the `extends` chain at index time.
        // Per-instance orientation rotates+mirrors every primitive
        // at the rect level so placement-rotation shows visually,
        // not just on the port positions.
        let orientation = crate::icon_paint::IconOrientation {
            rotation_deg: self.rotation_deg,
            mirror_x: self.mirror_x,
            mirror_y: self.mirror_y,
        };
        let mut drew_icon = false;
        if let Some(icon) = &self.icon_graphics {
            let sub = crate::icon_paint::TextSubstitution {
                name: (!self.instance_name.is_empty()).then_some(self.instance_name.as_str()),
                class_name: (!self.class_name.is_empty()).then_some(self.class_name.as_str()),
                parameters: (!self.parameters.is_empty()).then_some(self.parameters.as_slice()),
            };
            // Build a per-instance value resolver for MLS §18
            // `DynamicSelect` text expressions. The icon expression
            // is written in the component's local scope (`m`,
            // `port.m_flow`); the live snapshot is keyed by full
            // instance path (`tank.m`, `tank.port.m_flow`). We
            // prefix with `instance_name.` and look it up — that
            // covers both top-level state vars and dotted refs into
            // sub-components / ports. Falls back to the bare name
            // for cases like global `time`.
            let node_state =
                lunco_viz::kinds::canvas_plot_node::fetch_node_state(ctx.ui.ctx());
            let instance = self.instance_name.clone();
            let resolver = move |name: &str| -> Option<f64> {
                if !instance.is_empty() {
                    let qualified = format!("{instance}.{name}");
                    if let Some(&v) = node_state.values.get(&qualified) {
                        return Some(v);
                    }
                }
                node_state.values.get(name).copied()
            };
            let resolver_ref: &dyn Fn(&str) -> Option<f64> = &resolver;
            let palette = modelica_icon_palette_from_ctx(ctx.ui.ctx());
            // Source coord system: prefer the icon's *graphics* bbox
            // (visible body, excluding labels) so the icon body fills
            // the placement instead of leaving 30–50 % empty padding
            // around it. MSL convention is to author at -100..100, but
            // many components actually draw at -50..50 / -60..60 etc.,
            // which makes them look small inside the standard
            // placement. Excluding text from the bbox is intentional:
            // the body should fill the rect; labels drift slightly
            // outside but get clipped by the canvas widget. Falls
            // back to the declared coord system when there are no
            // graphics.
            let coord_system_for_paint = icon
                .graphics_bbox()
                .map(|e| crate::annotations::CoordinateSystem { extent: e })
                .unwrap_or(icon.coordinate_system);
            crate::icon_paint::paint_graphics_themed(
                painter,
                rect,
                coord_system_for_paint,
                orientation,
                Some(&sub),
                Some(resolver_ref),
                palette.as_ref(),
                &icon.graphics,
            );
            drew_icon = true;
            // MLS §18: when a class is rendered as a component
            // instance, only its Icon annotation is used. The Diagram
            // annotation (sub-components, internal wiring, pedagogical
            // labels like Torque's "Angle" box) belongs to the
            // class's editing view, never to instance rendering.
        }

        if !drew_icon {
            // Placeholder for classes with literally no `Icon` in
            // their extends chain — same shape as OMEdit's "no icon
            // authored yet" stand-in: rounded card + class name
            // centred. Once the user (or the indexer) authors an
            // Icon annotation, the live path above takes over and
            // we never run this fallback again.
            painter.rect_filled(rect, 6.0, theme_snap.card_fill);
            if !self.type_label.is_empty() && rect.height() > 30.0 {
                painter.text(
                    egui::pos2(rect.center().x, rect.center().y),
                    egui::Align2::CENTER_CENTER,
                    &self.type_label,
                    egui::FontId::proportional(10.0),
                    theme_snap.type_label,
                );
            }
        }

        // Border policy:
        //   - Selection ring: always drawn (functional feedback).
        //   - Icon-only / expandable connector accents: always drawn
        //     (carry semantic info — "decorative" / "expandable").
        //   - Placeholder card outline: always drawn (the card has
        //     no other body and would melt into the canvas otherwise).
        //   - Authored-icon hairline: opt-in via the theme snapshot's
        //     `show_authored_icon_border` flag, off by default. The
        //     icon's own primitives carry its bounds; the workbench
        //     hairline competed with them and was reported as visual
        //     noise. Power users can flip the flag in Settings later.
        let stroke = if selected {
            Some(egui::Stroke::new(2.0, theme_snap.select_stroke))
        } else if self.icon_only {
            Some(egui::Stroke::new(1.0, theme_snap.icon_only_stroke))
        } else if self.expandable_connector {
            Some(egui::Stroke::new(1.5, theme_snap.select_stroke))
        } else if !drew_icon {
            Some(egui::Stroke::new(1.0, theme_snap.inactive_stroke))
        } else if theme_snap.show_authored_icon_border {
            let c = theme_snap.inactive_stroke;
            let dim = egui::Color32::from_rgba_unmultiplied(
                c.r(),
                c.g(),
                c.b(),
                (c.a() / 3).max(40),
            );
            Some(egui::Stroke::new(0.75, dim))
        } else {
            None
        };
        if let Some(stroke) = stroke {
            let wants_dashed = (self.icon_only || self.expandable_connector) && !selected;
            if wants_dashed {
                paint_dashed_rect(painter, rect, 6.0, stroke);
            } else {
                painter.rect_stroke(rect, 6.0, stroke, egui::StrokeKind::Outside);
            }
        }

        // Instance name: deliberately NOT drawn here. Modelica icons
        // author their own `Text(textString="%name", extent={...})`
        // primitive — we substitute via `TextSubstitution` and the
        // icon decides where the name belongs. Drawing a workbench-
        // owned label here too produced the duplicate-name visual
        // noise users hit on the PID example. OMEdit / Dymola don't
        // draw an external label either.

        // Ports — shape per connector causality (OMEdit / Dymola
        // convention):
        //   • input  → filled square   (RealInput, BooleanInput, …)
        //   • output → filled triangle pointing outward
        //   • acausal physical → filled circle (Pin, Flange, HeatPort, …)
        // Direction is derived from where the port sits on the icon
        // boundary, classified the same way edges classify port_dir.
        //
        for port in &node.ports {
            let world = CanvasPos::new(
                node.rect.min.x + port.local_offset.x,
                node.rect.min.y + port.local_offset.y,
            );
            let p = ctx.viewport.world_to_screen(world, ctx.screen_rect);
            // Pixel-snap so the marker centre aligns with the
            // wire endpoint (which is also snapped — see
            // `EdgesLayer::draw`). Without this, the wire end
            // and the port circle drift apart by up to 1 px on
            // some zoom levels.
            let center = egui::pos2(p.x.round(), p.y.round());

            let cx = node.rect.min.x + node.rect.width() * 0.5;
            let cy = node.rect.min.y + node.rect.height() * 0.5;
            let dir = port_edge_dir(world.x - cx, world.y - cy);

            // Try the OMEdit-parity path first: render the connector
            // class's authored `Icon` at the port location. Falls
            // through to the generic per-shape marker if the class
            // can't be resolved (rare — typically only when the MSL
            // pre-warm hasn't reached that connector yet) or the
            // class has no `Icon` annotation in its inheritance chain.
            let port_info = self
                .port_connector_paths
                .iter()
                .find(|(name, _, _, _, _)| name == port.id.as_str());
            let connector_path: &str = port_info
                .map(|(_, p, _, _, _)| p.as_str())
                .unwrap_or("");
            let (port_size_x_icon, port_size_y_icon, port_rotation_deg) = port_info
                .map(|(_, _, sx, sy, rot)| (*sx, *sy, *rot))
                .unwrap_or((20.0, 20.0, 0.0));
            let mut painted_authored = false;
            // The indexer ideally writes a fully-qualified path, but
            // older indexes wrote the type as-declared (`"RealInput"`)
            // — fall back to a scope-chain walk rooted at the parent
            // class so cached indexes still resolve. First hit wins.
            let parent_qualified = self.parent_qualified_type.as_str();
            let candidates: Vec<String> = if connector_path.contains('.') {
                vec![connector_path.to_string()]
            } else if !connector_path.is_empty() {
                let mut out = Vec::new();
                let mut scope = parent_qualified.to_string();
                while scope.contains('.') {
                    let pkg = crate::ast_extract::parent_qualified(&scope).to_string();
                    if !pkg.is_empty() {
                        out.push(format!("{pkg}.Interfaces.{connector_path}"));
                        out.push(format!("{pkg}.{connector_path}"));
                    }
                    scope = pkg;
                }
                out.push(connector_path.to_string());
                out
            } else {
                Vec::new()
            };
            // Paint stays lock-free: the connector class's `Icon`
            // was pre-resolved off-thread in `project_scene` and
            // baked into `port_connector_icons` (parallel to
            // `port_connector_paths`). Read it by index. MLS §18:
            // the connector's authored Icon is what carries the
            // input/output triangle / acausal flange dot — drawing
            // it at the port location is what gives the diagram
            // its OMEdit-parity arrowheads, not a wire-end marker.
            let _ = candidates; // legacy fallback kept compiling; no longer used.
            let resolved_icon: Option<crate::annotations::Icon> = self
                .port_connector_paths
                .iter()
                .position(|(name, _, _, _, _)| name == port.id.as_str())
                .and_then(|i| self.port_connector_icons.get(i).cloned().flatten());
            if let Some(icon) = resolved_icon {
                {
                        // Render the connector's icon at the port
                        // location, sized to the port's authored
                        // `Placement(extent=...)` in the parent's icon
                        // coords. MSL convention: parent icon coord
                        // system spans 200 units (-100..100) and the
                        // parent is placed at `node.rect` in world
                        // coords. So 1 icon-unit = node_world / 200.
                        // Connector placement (e.g. Flange_a's
                        // 20×20 box) maps to 20/200 * node_world =
                        // 10% of the parent's world width — the small
                        // dot OMEdit shows.
                        let parent_w = node.rect.width().max(1.0);
                        let parent_h = node.rect.height().max(1.0);
                        // Use the authored placement extent as-is for
                        // every connector class — that is the size MSL
                        // authors intended (Flange_a's 20×20 dot, the
                        // 20×20 RealInput triangle on plain blocks, the
                        // 40×40 RealInput on LimPID). OMEdit / Dymola
                        // render at this size; over-scaling produces a
                        // triangle that dominates the icon body.
                        let half_x = (port_size_x_icon * 0.5 / 100.0) * (parent_w * 0.5);
                        let half_y = (port_size_y_icon * 0.5 / 100.0) * (parent_h * 0.5);
                        let world_rect = lunco_canvas::Rect::from_min_max(
                            lunco_canvas::Pos::new(world.x - half_x, world.y - half_y),
                            lunco_canvas::Pos::new(world.x + half_x, world.y + half_y),
                        );
                        let s_rect = ctx.viewport.world_rect_to_screen(world_rect, ctx.screen_rect);
                        let port_rect = egui::Rect::from_min_max(
                            egui::pos2(s_rect.min.x, s_rect.min.y),
                            egui::pos2(s_rect.max.x, s_rect.max.y),
                        );
                        let palette = modelica_icon_palette_from_ctx(ctx.ui.ctx());
                        // Compose the connector icon's orientation from
                        // (a) the parent's mirror flags so a mirrored
                        // parent (e.g. `extent={{22,-50},{2,-30}}` on
                        // speedSensor) flips the connector icon too —
                        // RealOutput's TIP must point AWAY from the
                        // parent regardless of which canvas side it
                        // ends up on, and (b) the port's authored
                        // `Placement(transformation(rotation=...))` so
                        // a `rotation=270` input sits with its
                        // triangle pointing the right way (e.g. PI's
                        // `u_m` on the bottom edge points up).
                        // MLS `rotation=270` on a port placement means
                        // 270° CCW *in the visual frame* (where Y is
                        // down, i.e. screen frame) — rotation=270 on
                        // PI's `u_m` produces a triangle pointing UP
                        // on screen. Our `to_screen` applies rotation
                        // in Modelica's +Y-up frame and then flips Y,
                        // which is equivalent to rotating CW in the
                        // visual frame. Negate so the visual outcome
                        // matches MLS / OMEdit.
                        // Include the PARENT's rotation in the port
                        // marker's orientation. Without this, when
                        // the parent is rotated (e.g. addSat at
                        // rotation=270), only the port POSITION is
                        // rotated — the connector arrow keeps its
                        // default orientation and ends up pointing
                        // the wrong way relative to the rotated
                        // icon. Adding the parent's rotation makes
                        // the marker rotate WITH the icon body so
                        // the arrow tip always points into the icon.
                        let port_orientation = crate::icon_paint::IconOrientation {
                            rotation_deg: self.rotation_deg - port_rotation_deg,
                            mirror_x: self.mirror_x,
                            mirror_y: self.mirror_y,
                        };
                        crate::icon_paint::paint_graphics_themed(
                            painter,
                            port_rect,
                            icon.coordinate_system,
                            port_orientation,
                            None,
                            None,
                            palette.as_ref(),
                            &icon.graphics,
                        );
                        painted_authored = true;
                    }
            }

            if !painted_authored {
                // Generic fallback for unresolved connectors / classes
                // that ship no `Icon` annotation.
                let shape = match port.kind.as_str() {
                    "input" => PortShape::InputSquare,
                    "output" => PortShape::OutputTriangle,
                    _ => PortShape::AcausalCircle,
                };
                let fill = theme_snap.port_fill;
                let scale = (ctx.viewport.zoom / 3.0).sqrt().clamp(0.7, 1.4);
                let stroke = egui::Stroke::new(0.6 * scale, theme_snap.port_stroke);
                paint_port_shape(painter, center, shape, dir, fill, stroke, scale);
            }
        }

        // Hover tooltip. The canvas claims the whole widget rect
        // with `Sense::click_and_drag()` so `ui.interact(.., Sense::hover())`
        // and even `show_tooltip_at_pointer` get suppressed at the
        // visual's layer. Paint the tooltip card directly with the
        // foreground painter — bypasses egui's interaction layering
        // entirely.
        let cursor = ctx.ui.ctx().pointer_hover_pos();
        // Suppress the tooltip when the cursor isn't actually over
        // the canvas (e.g. floated past the widget edge while still
        // hovering the icon's *world rect*). Without this the card
        // can sit on top of the side panels because it paints in
        // an unclipped layer.
        let canvas_widget_rect = ctx.ui.max_rect();
        let in_canvas = cursor
            .map(|c| canvas_widget_rect.contains(c))
            .unwrap_or(false);
        let is_hovered = cursor
            .map(|c| rect.contains(c))
            .unwrap_or(false)
            && in_canvas;
        if is_hovered && !self.instance_name.is_empty() {
            let cursor = cursor.unwrap();
            let snap =
                lunco_viz::kinds::canvas_plot_node::fetch_node_state(
                    ctx.ui.ctx(),
                );
            let prefix = format!("{}.", self.instance_name);
            let mut rows: Vec<(&String, &f64)> = snap
                .values
                .iter()
                .filter(|(k, _)| k.starts_with(&prefix))
                .collect();
            rows.sort_by(|a, b| a.0.cmp(b.0));
            paint_hover_card(
                ctx.ui,
                cursor,
                &self.instance_name,
                &self.class_name,
                &rows,
            );
        }

        // Dashboard-style in-canvas control widget. Last call in
        // draw so the painter borrow taken above has ended (Rust
        // NLL allows ui to be reborrowed mutably here for
        // `ui.interact`). The widget is always visible while the
        // icon is rendered and captures pointer events itself so
        // dragging the slider does NOT also drag the node.
        paint_input_control_widget(ctx.ui, rect, &self.instance_name, ctx.viewport.zoom);
    }
    fn debug_name(&self) -> &str {
        "modelica.icon"
    }
}

/// Direct-paint hover card (foreground layer). Used because the
/// canvas's `Sense::click_and_drag()` swallows ordinary tooltip
/// hooks at the visual layer.
pub(super) fn paint_hover_card(
    ui: &mut egui::Ui,
    cursor: egui::Pos2,
    instance: &str,
    class_name: &str,
    rows: &[(&String, &f64)],
) {
    let theme = lunco_theme::active(ui.ctx());
    let overlay_shadow = theme.colors.base.alpha(110);
    let overlay_fill = theme.colors.surface0;
    let overlay_stroke = theme.colors.surface2;
    let overlay_text = theme.tokens.text;
    let layer_id = egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new(("modelica_icon_hover_card", instance)),
    );
    let painter = ui.ctx().layer_painter(layer_id);
    // Clip to the canvas widget rect so the card never paints over
    // the side panels (the user would otherwise see a tooltip
    // ghost overlapping the Twin Browser when hovering an icon
    // near the canvas's left edge).
    let canvas_clip = ui.max_rect();
    let painter = painter.with_clip_rect(canvas_clip);

    // Build text lines first so we can size the card accordingly.
    let mut lines: Vec<(String, bool)> = Vec::with_capacity(rows.len() + 4);
    lines.push((instance.to_string(), true));
    if !class_name.is_empty() {
        lines.push((class_name.to_string(), false));
    }
    if rows.is_empty() {
        lines.push(("(no values yet — run a sim)".to_string(), false));
    } else {
        for (k, v) in rows {
            let short = k.strip_prefix(&format!("{instance}.")).unwrap_or(k);
            lines.push((format!("{short:<10}  {v:>10.4}"), false));
        }
    }

    let line_h = 14.0_f32;
    let pad = 6.0_f32;
    // Estimate width: 7 px per char (monospace). egui doesn't expose
    // `Painter::text_size` cheaply; this is plenty for the typical
    // path widths we render.
    let text_w = lines
        .iter()
        .map(|(s, _)| s.chars().count() as f32 * 7.0)
        .fold(0.0_f32, f32::max);
    let card_w = (text_w + pad * 2.0).clamp(120.0, 360.0);
    let card_h = lines.len() as f32 * line_h + pad * 2.0;

    // Anchor card to the right of the cursor with a small offset;
    // flip to the left if we'd run off the screen edge.
    let screen = ui.ctx().content_rect();
    let mut origin =
        egui::pos2(cursor.x + 14.0, cursor.y + 14.0);
    if origin.x + card_w > screen.max.x {
        origin.x = cursor.x - card_w - 14.0;
    }
    if origin.y + card_h > screen.max.y {
        origin.y = cursor.y - card_h - 14.0;
    }
    let card_rect = egui::Rect::from_min_size(
        origin,
        egui::vec2(card_w, card_h),
    );
    // Drop shadow so the card pops over the diagram.
    painter.rect_filled(
        card_rect.translate(egui::vec2(0.0, 2.0)),
        6.0,
        overlay_shadow,
    );
    painter.rect_filled(card_rect, 6.0, overlay_fill);
    painter.rect_stroke(
        card_rect,
        6.0,
        egui::Stroke::new(1.0, overlay_stroke),
        egui::StrokeKind::Outside,
    );

    let mut y = origin.y + pad;
    for (line, is_title) in &lines {
        let font = if *is_title {
            egui::FontId::proportional(13.0)
        } else {
            egui::FontId::monospace(11.0)
        };
        let color = if *is_title {
            overlay_text
        } else {
            overlay_text.gamma_multiply(0.85)
        };
        painter.text(
            egui::pos2(origin.x + pad, y),
            egui::Align2::LEFT_TOP,
            line,
            font,
            color,
        );
        y += line_h;
    }
}

/// Paint a chain of small bright dots along a polyline that march
/// from the first to the last vertex at constant screen-pixel speed.
/// Phase keyed off wall-clock `time` so all wires stay in sync.
/// Used as the "this connection is alive" overlay during simulation
/// — Simulink/SPICE-style, no per-edge flow data needed yet.
pub(super) fn paint_flow_dots(
    painter: &egui::Painter,
    polyline: &[egui::Pos2],
    base_color: egui::Color32,
    time: f64,
    scale: f32,
) {
    if polyline.len() < 2 {
        return;
    }
    let mut total_len = 0.0_f32;
    for w in polyline.windows(2) {
        total_len += (w[1] - w[0]).length();
    }
    if total_len < 1.0 {
        return;
    }
    // Spacing + speed in screen pixels. Tuned iteratively: 64 px
    // looked empty; 28 px read as a dotted wire ("bumpy"); 32 px
    // was OK but still felt sparse on long runs; 22 px was better
    // but on short wire segments (a half-inch fluid line between
    // valve.port_b and engine.port) only 1–2 dots were ever
    // visible at one phase, so during the animation cycle the
    // wire spent most of its time looking static. 16 px gives
    // every short segment at least 3–4 dots in flight, so the
    // motion cue is always visible. Alpha stays moderate (180)
    // so the dots read as a moving stream rather than a solid
    // dotted line.
    // Spacing/speed scale strictly with canvas zoom so the dots are
    // anchored to *world* distance: at 2× zoom they move twice as
    // fast on screen but cover the same wire length per second.
    const SPACING_PX: f32 = 16.0;
    const SPEED_PX_S: f32 = 36.0;
    let spacing = SPACING_PX * scale;
    let speed = SPEED_PX_S * scale;
    let phase = ((time as f32) * speed).rem_euclid(spacing);
    let dot_color = egui::Color32::from_rgba_unmultiplied(
        base_color.r(),
        base_color.g(),
        base_color.b(),
        180,
    );
    let mut s = phase;
    while s < total_len {
        // Walk the polyline to find the segment containing arc-length s.
        let mut acc = 0.0_f32;
        for w in polyline.windows(2) {
            let seg_len = (w[1] - w[0]).length();
            if s <= acc + seg_len {
                let t = ((s - acc) / seg_len).clamp(0.0, 1.0);
                let p = w[0] + (w[1] - w[0]) * t;
                // Slightly larger radius (was 2.2 × scale) so
                // the dot is unambiguous at low canvas zoom.
                painter.circle_filled(p, 2.6 * scale, dot_color);
                break;
            }
            acc += seg_len;
        }
        s += spacing;
    }
}

