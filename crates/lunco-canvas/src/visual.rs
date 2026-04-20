//! Extension seams for rendering individual scene elements.
//!
//! [`NodeVisual`] and [`EdgeVisual`] are the two per-element trait
//! objects the canvas renders through. Each scene [`crate::scene::Node`]
//! and [`crate::scene::Edge`] stores a string `kind` + opaque
//! `data: serde_json::Value`; at load / after-edit time the canvas
//! looks up the kind in a [`VisualRegistry`] to (re)build the trait
//! object.
//!
//! The point of the indirection: `Box<dyn NodeVisual>` can't be
//! serialised, but its `kind` + the struct it deserialises from can.
//! When a `.lcscene` is loaded or a `Scene` snapshot is restored for
//! undo, the registry reconstructs the visuals in one pass.
//!
//! # What's NOT in this module
//!
//! Concrete visuals (Modelica icon, Bezier edge, snarl-style port
//! block, animated wire, widget-in-node) all live in domain crates.
//! `lunco-canvas` ships only the traits and the registry.
//!
//! # `DrawCtx` design
//!
//! `DrawCtx` carries `&mut egui::Ui` (not just `Painter`) + `time` +
//! `extras: &dyn Any`. The Ui reference lets a future widget-in-node
//! visual allocate a child UI at the transformed rect — that's how
//! "embed a live plot inside a component" works without any core
//! change. `extras` is the viz-overlay escape hatch (signal lookups,
//! per-frame decoration state) so new decorations land as a new
//! visual impl reading `extras.downcast_ref::<VizOverlayCtx>()`,
//! never a trait signature change.

use std::any::Any;
use std::collections::HashMap;

use bevy_egui::egui;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use smol_str::SmolStr;

use crate::scene::{Node, PortId, Pos, Rect};
use crate::viewport::Viewport;

/// Everything a visual's `draw` method sees.
///
/// Held by value so visuals can freely rebind inner fields; the two
/// mutable references (`ui`, `extras` is shared) are the only things
/// that actually alias the caller's state.
pub struct DrawCtx<'a> {
    /// The egui UI the canvas is painting into — pass to
    /// `allocate_ui_at_rect` if the visual needs to embed egui widgets
    /// (e.g. a live plot inside a node body).
    pub ui: &'a mut egui::Ui,
    /// Current viewport — visuals use it for level-of-detail
    /// decisions (e.g. hide tiny labels when zoomed out) or for
    /// computing screen-rect edge endpoints.
    pub viewport: &'a Viewport,
    /// Widget-local screen rect the canvas is drawing into.
    /// `viewport.world_to_screen` takes this as the screen_rect arg.
    pub screen_rect: Rect,
    /// Continuous animation clock in seconds. Visuals that animate
    /// (flowing dashes, pulse) read this directly.
    pub time: f64,
    /// Escape hatch for decoration layers (viz overlay, signal
    /// lookups, anything future). Concrete visuals downcast:
    /// `ctx.extras.downcast_ref::<VizOverlayCtx>()`.
    pub extras: &'a dyn Any,
}

/// Hit-test result returned by [`NodeVisual::hit`] — what part of the
/// node the pointer is over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeHit {
    /// The node's body (drag → move the node).
    Body,
    /// A named port (drag → start an edge from this port).
    Port(PortId),
}

/// How a node is rendered and hit-tested.
///
/// Stateless per frame: the visual's struct holds *configuration*
/// (icon SVG, theme choices) not *animation state*. Per-frame state
/// comes from `DrawCtx.time` / `DrawCtx.extras`. This keeps visuals
/// cheap to clone and safe to share across scenes.
pub trait NodeVisual: Send + Sync {
    /// Paint the node. `node.rect` is in world coordinates — the
    /// visual transforms via `ctx.viewport` before issuing shapes.
    fn draw(&self, ctx: &mut DrawCtx, node: &Node, selected: bool);

    /// Return which part of the node (if any) `world_pos` intersects.
    /// Default impl treats the whole `node.rect` as body and each
    /// port as a 6-world-unit circle around its local offset —
    /// enough for simple visuals to work without overriding.
    fn hit(&self, node: &Node, world_pos: Pos) -> Option<NodeHit> {
        // Ports get priority — a port on the boundary of the body
        // should hit as a port, not as the body.
        for port in &node.ports {
            let px = node.rect.min.x + port.local_offset.x;
            let py = node.rect.min.y + port.local_offset.y;
            let dx = world_pos.x - px;
            let dy = world_pos.y - py;
            if dx * dx + dy * dy <= 36.0 {
                // 6 world-unit radius
                return Some(NodeHit::Port(port.id.clone()));
            }
        }
        if node.rect.contains(world_pos) {
            Some(NodeHit::Body)
        } else {
            None
        }
    }

    /// Debug/telemetry name. Defaults to the kind-id. Usually no
    /// reason to override.
    fn debug_name(&self) -> &str {
        "node"
    }
}

/// How an edge is rendered and hit-tested.
///
/// `from_screen` / `to_screen` are precomputed by the canvas — the
/// edge visual doesn't need to look up the nodes. Keeps the trait
/// self-contained.
pub trait EdgeVisual: Send + Sync {
    fn draw(
        &self,
        ctx: &mut DrawCtx,
        from_screen: Pos,
        to_screen: Pos,
        selected: bool,
    );

    /// True if `world_pos` is close enough to the edge's curve to
    /// count as a click on it. Canvas uses this for click-to-select
    /// and for delete-key target resolution.
    ///
    /// Default impl is a 4-world-unit straight-line perpendicular
    /// distance check — covers bezier and straight edges adequately
    /// for selection; exotic shapes override.
    fn hit(&self, world_pos: Pos, from_world: Pos, to_world: Pos) -> bool {
        perpendicular_dist_sq(world_pos, from_world, to_world) <= 16.0
    }
}

/// Squared perpendicular distance from `p` to the finite segment
/// `(a,b)`. Endpoint-clamped — so clicking past the end of an edge
/// doesn't count as a hit.
fn perpendicular_dist_sq(p: Pos, a: Pos, b: Pos) -> f32 {
    let ax = b.x - a.x;
    let ay = b.y - a.y;
    let len_sq = ax * ax + ay * ay;
    if len_sq < f32::EPSILON {
        // Degenerate segment — treat as point distance to a.
        let dx = p.x - a.x;
        let dy = p.y - a.y;
        return dx * dx + dy * dy;
    }
    // Projection parameter t ∈ [0,1] of p onto segment.
    let t = (((p.x - a.x) * ax + (p.y - a.y) * ay) / len_sq).clamp(0.0, 1.0);
    let foot_x = a.x + t * ax;
    let foot_y = a.y + t * ay;
    let dx = p.x - foot_x;
    let dy = p.y - foot_y;
    dx * dx + dy * dy
}

/// Factory closure that builds a trait object from a node's
/// `(data, kind)` pair. Stored in the registry keyed by kind.
pub type NodeVisualFactory = Box<dyn Fn(&JsonValue) -> Box<dyn NodeVisual> + Send + Sync>;
pub type EdgeVisualFactory = Box<dyn Fn(&JsonValue) -> Box<dyn EdgeVisual> + Send + Sync>;

/// Per-app registry of visual kinds. Domain crates call
/// [`VisualRegistry::register_node_kind`] at plugin-build time; the
/// canvas looks up by `Node::kind` when it needs to construct a
/// visual.
///
/// Registrations are fallible at *resolve* time (unknown kind) — this
/// is deliberate: a bad kind should fail loudly at the point a
/// specific node couldn't be drawn, not silently at registry
/// configuration.
#[derive(Default)]
pub struct VisualRegistry {
    nodes: HashMap<SmolStr, NodeVisualFactory>,
    edges: HashMap<SmolStr, EdgeVisualFactory>,
}

impl VisualRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_node_kind<F, V>(&mut self, kind: impl Into<SmolStr>, factory: F)
    where
        F: Fn(&JsonValue) -> V + Send + Sync + 'static,
        V: NodeVisual + 'static,
    {
        self.nodes.insert(
            kind.into(),
            Box::new(move |data| Box::new(factory(data))),
        );
    }

    pub fn register_edge_kind<F, V>(&mut self, kind: impl Into<SmolStr>, factory: F)
    where
        F: Fn(&JsonValue) -> V + Send + Sync + 'static,
        V: EdgeVisual + 'static,
    {
        self.edges.insert(
            kind.into(),
            Box::new(move |data| Box::new(factory(data))),
        );
    }

    pub fn build_node(&self, kind: &str, data: &JsonValue) -> Option<Box<dyn NodeVisual>> {
        self.nodes.get(kind).map(|f| f(data))
    }
    pub fn build_edge(&self, kind: &str, data: &JsonValue) -> Option<Box<dyn EdgeVisual>> {
        self.edges.get(kind).map(|f| f(data))
    }

    pub fn has_node_kind(&self, kind: &str) -> bool {
        self.nodes.contains_key(kind)
    }
    pub fn has_edge_kind(&self, kind: &str) -> bool {
        self.edges.contains_key(kind)
    }
}

/// A minimal placeholder node visual — drawn as a rectangle with
/// the node's label. Ships with the crate so tests and first-run
/// apps have *something* to render before any domain visual is
/// registered. Domain crates override by registering over the
/// `"lunco.canvas.placeholder"` kind.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlaceholderNodeVisual;

impl NodeVisual for PlaceholderNodeVisual {
    fn draw(&self, ctx: &mut DrawCtx, node: &Node, selected: bool) {
        let screen_rect = ctx
            .viewport
            .world_rect_to_screen(node.rect, ctx.screen_rect);
        let rect = egui::Rect::from_min_max(
            egui::pos2(screen_rect.min.x, screen_rect.min.y),
            egui::pos2(screen_rect.max.x, screen_rect.max.y),
        );
        let painter = ctx.ui.painter();
        let fill = if selected {
            egui::Color32::from_rgb(58, 82, 120)
        } else {
            egui::Color32::from_rgb(40, 46, 58)
        };
        let stroke_col = if selected {
            egui::Color32::from_rgb(120, 170, 255)
        } else {
            egui::Color32::from_rgb(90, 100, 120)
        };
        painter.rect_filled(rect, 4.0, fill);
        painter.rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(1.5, stroke_col),
            egui::StrokeKind::Outside,
        );
        if !node.label.is_empty() {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                &node.label,
                egui::FontId::proportional(12.0),
                egui::Color32::from_rgb(220, 220, 225),
            );
        }
    }
    fn debug_name(&self) -> &str {
        "placeholder"
    }
}

/// A minimal placeholder edge — a thin straight line. Same role as
/// [`PlaceholderNodeVisual`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlaceholderEdgeVisual;

impl EdgeVisual for PlaceholderEdgeVisual {
    fn draw(
        &self,
        ctx: &mut DrawCtx,
        from_screen: Pos,
        to_screen: Pos,
        selected: bool,
    ) {
        let col = if selected {
            egui::Color32::from_rgb(120, 170, 255)
        } else {
            egui::Color32::from_rgb(150, 150, 160)
        };
        ctx.ui.painter().line_segment(
            [
                egui::pos2(from_screen.x, from_screen.y),
                egui::pos2(to_screen.x, to_screen.y),
            ],
            egui::Stroke::new(if selected { 2.0 } else { 1.5 }, col),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Port, PortId};

    fn mk_node() -> Node {
        Node {
            id: crate::scene::NodeId(0),
            rect: Rect::from_min_size(Pos::new(0.0, 0.0), 100.0, 50.0),
            kind: "test".into(),
            data: JsonValue::Null,
            ports: vec![
                Port {
                    id: PortId::new("in"),
                    local_offset: Pos::new(0.0, 25.0),
                    kind: "".into(),
                },
                Port {
                    id: PortId::new("out"),
                    local_offset: Pos::new(100.0, 25.0),
                    kind: "".into(),
                },
            ],
            label: String::new(),
            origin: None,
        }
    }

    struct FreshVisual;
    impl NodeVisual for FreshVisual {
        fn draw(&self, _c: &mut DrawCtx, _n: &Node, _s: bool) {}
    }

    #[test]
    fn default_hit_prefers_port_over_body() {
        let v = FreshVisual;
        let n = mk_node();
        // Right on the "out" port.
        assert_eq!(
            v.hit(&n, Pos::new(100.0, 25.0)),
            Some(NodeHit::Port(PortId::new("out")))
        );
        // Inside body, away from any port.
        assert_eq!(v.hit(&n, Pos::new(50.0, 25.0)), Some(NodeHit::Body));
        // Outside.
        assert_eq!(v.hit(&n, Pos::new(200.0, 25.0)), None);
    }

    #[test]
    fn port_hit_is_within_radius_6() {
        let v = FreshVisual;
        let n = mk_node();
        // 5 units from the "in" port → hits port.
        assert_eq!(
            v.hit(&n, Pos::new(3.0, 25.0 + 4.0)),
            Some(NodeHit::Port(PortId::new("in")))
        );
        // 7 units from port but inside body → hits body.
        assert_eq!(v.hit(&n, Pos::new(7.0, 25.0)), Some(NodeHit::Body));
    }

    #[test]
    fn edge_default_hit_rejects_off_line_points() {
        let v = PlaceholderEdgeVisual;
        let a = Pos::new(0.0, 0.0);
        let b = Pos::new(100.0, 0.0);
        // Right on the line.
        assert!(v.hit(Pos::new(50.0, 0.0), a, b));
        // 3 units off — within threshold of 4.
        assert!(v.hit(Pos::new(50.0, 3.0), a, b));
        // 5 units off — outside.
        assert!(!v.hit(Pos::new(50.0, 5.0), a, b));
        // Beyond endpoint (clamped) — also outside.
        assert!(!v.hit(Pos::new(120.0, 0.0), a, b));
    }

    #[test]
    fn registry_roundtrips_a_kind() {
        let mut reg = VisualRegistry::new();
        reg.register_node_kind("test.placeholder", |_| PlaceholderNodeVisual);
        assert!(reg.has_node_kind("test.placeholder"));
        let v = reg.build_node("test.placeholder", &JsonValue::Null);
        assert!(v.is_some());
        assert!(reg.build_node("test.unknown", &JsonValue::Null).is_none());
    }
}
