//! Interaction trait — what turns mouse/keyboard events into
//! scene mutations.
//!
//! Exactly one tool is **active** at a time; the canvas dispatches
//! each [`InputEvent`](crate::event::InputEvent) to the active tool,
//! which decides whether to consume it (start a drag, begin an edge
//! connection) or let built-in navigation handle it (pan, zoom).
//!
//! # Shipping today: [`DefaultTool`]
//!
//! Handles the Modelica use case — click-select, shift-extend, drag-
//! to-move, drag-from-port to connect, Delete key, double-click for
//! drill-in. Custom tools (annotation brush, schematic autoroute,
//! typed-port-validate) become additional impls when the real
//! requirement lands; no change to the canvas is needed.
//!
//! # `CanvasOps` façade
//!
//! Tools don't get a `&mut Canvas`. They get a narrow `CanvasOps`
//! facade that lets them mutate the scene / selection / viewport
//! and emit [`SceneEvent`](crate::event::SceneEvent)s, but not reach
//! for layers/overlays/other tools. Makes tool impls independently
//! testable and protects the canvas from tool-side invariant breaks.
//!
//! # Outcome
//!
//! `handle` returns [`ToolOutcome`] — whether the event was
//! *consumed* (built-in navigation should skip it) or *passed
//! through* (navigation handles it). This is how tool authors let
//! the canvas deal with "boring" events without reimplementing
//! pan/zoom themselves.
//!
//! # Why the state machine is explicit
//!
//! `DefaultTool` tracks an enum of idle/pressed/dragging-node/
//! connecting states. Rolling these into ad-hoc bools ("are we
//! dragging?") makes corner cases (a mouse-up outside the widget,
//! a modifier change mid-drag) subtle; an enum forces every
//! transition to be deliberate. Unit tests below drive the machine
//! through the full interaction arcs.

use std::collections::HashMap;

use crate::event::{
    ContextTarget, InputEvent, MouseButton, SceneEvent,
};
use crate::scene::{NodeHitKind, NodeId, PortRef, Pos, Rect, Scene};
use crate::selection::{SelectItem, Selection};
use crate::viewport::Viewport;

/// Result of a tool handling one event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOutcome {
    /// Tool consumed the event — built-in navigation should skip it.
    Consumed,
    /// Tool didn't care — navigation (pan/zoom/etc.) handles it.
    Passthrough,
}

/// Narrow mutable façade passed to tools. Deliberately does not
/// expose layers/overlays/other-tools — a tool's job is to mutate
/// authored state, emit events, and optionally change selection; it
/// has no business touching render plugins.
pub struct CanvasOps<'a> {
    pub scene: &'a mut Scene,
    pub selection: &'a mut Selection,
    pub viewport: &'a mut Viewport,
    pub events: &'a mut Vec<SceneEvent>,
    /// When `true`, tools must not mutate `scene` — no drag-to-move,
    /// no drag-to-connect, no delete-on-key. Pan/zoom/selection
    /// stay fine (those mutate `viewport` / `selection`, not the
    /// authored scene). Surfaced as a [`Canvas::read_only`] field
    /// that the embedding app flips per tab (e.g. MSL library tabs).
    pub read_only: bool,
    /// Optional drag-to-grid snap. When `Some`, the default tool
    /// quantises in-flight drag translations to multiples of `step`
    /// world units — user sees icons click into alignment *during*
    /// the drag, not just at commit. Plumbed down from
    /// [`crate::Canvas::snap`].
    pub snap: Option<crate::canvas::SnapSettings>,
}

/// Hit-test radius for ports, in world units. Chosen so clicking
/// near (but not exactly on) a port still latches on — matches the
/// "sloppy click" tolerance users expect from Dymola/similar.
const PORT_HIT_RADIUS: f32 = 6.0;
/// Click-vs-drag threshold in world units. A drag that covers less
/// than this distance is treated as a click (no scene mutation) to
/// avoid phantom 1-pixel moves when the user intended to click.
const DRAG_THRESHOLD_WORLD: f32 = 2.0;

/// What drives the canvas's interactive behaviour. See module docs.
pub trait Tool: Send + Sync {
    /// Called once per input event while this tool is active.
    fn handle(&mut self, event: &InputEvent, ops: &mut CanvasOps) -> ToolOutcome;

    /// Per-frame hook (even when no input). Default: noop. Used by
    /// tools with in-flight state — e.g. a rubber-band that needs
    /// the active pointer position to draw its preview, or an
    /// autoroute tool that animates a path preview.
    fn tick(&mut self, _ops: &mut CanvasOps, _dt: f32) {}

    /// Name used in debug output and (eventually) toolbar buttons.
    fn name(&self) -> &'static str;

    /// What the canvas should draw as a tool preview, if anything.
    /// Used by `ToolPreviewLayer`. Default: nothing. Implementers
    /// override to show ghost connections, rubber-band rectangles,
    /// drop-target glows.
    fn preview(&self) -> Option<ToolPreview> {
        None
    }

    /// Drop any in-flight gesture state (pressed-but-not-yet-dragging,
    /// in-flight node drag, in-flight edge connect, rubber-band).
    /// Called when the scene is wholesale-replaced under the tool's
    /// feet (e.g. after a re-projection completes), since the tool
    /// state holds `NodeId`s that no longer exist in the new scene.
    /// Default: noop — stateless tools are unaffected.
    fn cancel_in_flight(&mut self) {}

    /// Remap any stable references the tool is holding (typically
    /// `NodeId`s captured at press / drag-start time) when the scene
    /// is replaced. `find_new_id` is called for each old id; if it
    /// returns `Some(new_id)` the tool should rewrite its internal
    /// state to use it, otherwise drop that reference. Used by the
    /// host to preserve in-flight drags across re-projections.
    /// Default: noop.
    fn remap_node_ids(&mut self, _find_new_id: &dyn Fn(crate::scene::NodeId) -> Option<crate::scene::NodeId>) {}
}

/// What the tool wants drawn as a preview on top of the scene.
/// Kept small — exotic previews are always expressible as one of
/// these cases or a combination.
#[derive(Debug, Clone)]
pub enum ToolPreview {
    /// Ghost edge from a port to an in-flight pointer position. Used
    /// while dragging a new connection.
    GhostEdge {
        from_world: Pos,
        to_world: Pos,
        /// When the pointer is near a valid target port, set this so
        /// the layer can highlight the snap.
        snap_target: Option<Pos>,
    },
    /// Rubber-band selection rectangle, world-space.
    RubberBand(Rect),
}

// ─── DefaultTool ────────────────────────────────────────────────────

/// Interaction states. The enum is the single source of truth about
/// what the tool is currently doing; every event handler is a
/// transition between these.
#[derive(Debug, Clone)]
enum State {
    /// Nothing in flight. Hover state may still be active on the
    /// canvas for highlight purposes, but the tool itself is idle.
    Idle,

    /// Primary pressed — we don't know yet whether the user meant a
    /// click or the start of a drag. `origin_world` is the press
    /// position; we transition to one of the Dragging* variants the
    /// moment the pointer moves more than [`DRAG_THRESHOLD_WORLD`]
    /// from it.
    PrimaryPressed {
        origin_world: Pos,
        /// What was under the press point — determines which drag
        /// variant we promote to.
        landed_on: PressTarget,
        /// Modifier state at press time, not re-read during drag.
        extend: bool, // shift
        toggle: bool, // ctrl
    },

    /// Dragging one or more selected nodes. The tool remembers each
    /// node's original rect so it can compute cumulative translation
    /// and emit a clean `NodeMoved { old_min, new_min }` per node on
    /// mouse-up.
    DraggingNodes {
        origin_world: Pos,
        original_rects: HashMap<NodeId, Rect>,
    },

    /// Dragging from a port — creating a new edge. `from` is the
    /// origin port; `pointer_world` is the tip of the ghost edge,
    /// updated each PointerMove. On release over a valid target
    /// port we emit `EdgeCreated`.
    ConnectingFromPort {
        from: PortRef,
        from_world: Pos,
        pointer_world: Pos,
    },

    /// Rubber-band select. World-space rect from press-origin to
    /// current pointer. On release, add every node intersecting to
    /// the selection (respecting extend/toggle).
    RubberBand {
        origin_world: Pos,
        pointer_world: Pos,
        extend: bool,
        toggle: bool,
    },
}

/// Which scene element the primary press landed on — recorded at
/// press-time so the drag promotion knows what to do.
#[derive(Debug, Clone)]
enum PressTarget {
    NodeBody(NodeId),
    Port(PortRef, Pos), // port world-space position for ghost edge origin
    Empty,
}

/// Built-in Modelica/graph-editor tool: select, drag, connect, delete.
pub struct DefaultTool {
    state: State,
    /// Tracks the last observed pointer position in world coords so
    /// the preview layer can render ghost edges / rubber-bands from
    /// a consistent source of truth (rather than reaching into the
    /// state enum variants from the outside).
    last_pointer_world: Option<Pos>,
}

impl Default for DefaultTool {
    fn default() -> Self {
        Self {
            state: State::Idle,
            last_pointer_world: None,
        }
    }
}

impl Tool for DefaultTool {
    fn cancel_in_flight(&mut self) {
        self.state = State::Idle;
    }

    fn remap_node_ids(&mut self, find_new_id: &dyn Fn(crate::scene::NodeId) -> Option<crate::scene::NodeId>) {
        match &mut self.state {
            State::PrimaryPressed { landed_on, .. } => {
                match landed_on {
                    PressTarget::NodeBody(id) => {
                        if let Some(new_id) = find_new_id(*id) {
                            *id = new_id;
                        } else {
                            // Lost the node — fall back to "click on
                            // empty space". Drag will become a
                            // rubber-band, which is a less surprising
                            // failure than a no-op.
                            *landed_on = PressTarget::Empty;
                        }
                    }
                    PressTarget::Port(port_ref, _) => {
                        if let Some(new_id) = find_new_id(port_ref.node) {
                            port_ref.node = new_id;
                        } else {
                            *landed_on = PressTarget::Empty;
                        }
                    }
                    PressTarget::Empty => {}
                }
            }
            State::DraggingNodes { original_rects, .. } => {
                let remapped: HashMap<_, _> = std::mem::take(original_rects)
                    .into_iter()
                    .filter_map(|(old_id, rect)| {
                        find_new_id(old_id).map(|new_id| (new_id, rect))
                    })
                    .collect();
                *original_rects = remapped;
            }
            State::ConnectingFromPort { from, .. } => {
                if let Some(new_id) = find_new_id(from.node) {
                    from.node = new_id;
                } else {
                    self.state = State::Idle;
                }
            }
            _ => {}
        }
    }

    fn handle(&mut self, event: &InputEvent, ops: &mut CanvasOps) -> ToolOutcome {
        match event {
            InputEvent::PointerDown {
                button: MouseButton::Primary,
                world,
                modifiers,
                ..
            } => {
                self.last_pointer_world = Some(*world);
                self.on_primary_down(*world, modifiers.shift, modifiers.ctrl, ops);
                ToolOutcome::Consumed
            }

            InputEvent::PointerMove { world, .. } => {
                self.last_pointer_world = Some(*world);
                // A move might promote Pressed → Dragging, or feed
                // an in-flight drag. Pan (middle-drag) lives in
                // navigation so we don't consume when idle.
                if matches!(self.state, State::Idle) {
                    ToolOutcome::Passthrough
                } else {
                    self.on_pointer_move(*world, ops);
                    ToolOutcome::Consumed
                }
            }

            InputEvent::PointerUp {
                button: MouseButton::Primary,
                world,
                ..
            } => {
                self.last_pointer_world = Some(*world);
                self.on_primary_up(*world, ops);
                ToolOutcome::Consumed
            }

            InputEvent::DoubleClick { world, .. } => {
                // Double-click on a node body = "drill in" event. We
                // don't mutate the scene — the caller decides what
                // "drill in" means for its domain (open subclass tab
                // in Modelica, expand subgraph in dataflow, …).
                if let Some((id, NodeHitKind::Body)) =
                    ops.scene.hit_node(*world, PORT_HIT_RADIUS)
                {
                    ops.events.push(SceneEvent::NodeDoubleClicked { id });
                    return ToolOutcome::Consumed;
                }
                ToolOutcome::Passthrough
            }

            // Right-click is handled by the caller via
            // `Response::context_menu` on the canvas's Response. The
            // tool ignores secondary events entirely — trying to
            // detect right-click here was fragile and duplicated
            // what egui already does natively.
            InputEvent::PointerDown {
                button: MouseButton::Secondary,
                ..
            } => ToolOutcome::Passthrough,

            InputEvent::Key { name, .. } => match *name {
                "Delete" | "Backspace" => {
                    if ops.read_only {
                        // Block deletion on read-only canvases.
                        // Returning Consumed swallows the key so it
                        // doesn't trigger a fallback handler.
                        return ToolOutcome::Consumed;
                    }
                    self.delete_selection(ops);
                    ToolOutcome::Consumed
                }
                "Escape" => {
                    // Abort any in-flight drag/connect. Don't clear
                    // the selection — Escape in most editors cancels
                    // the interaction, not the selection.
                    self.state = State::Idle;
                    ToolOutcome::Consumed
                }
                _ => ToolOutcome::Passthrough,
            },

            _ => ToolOutcome::Passthrough,
        }
    }

    fn name(&self) -> &'static str {
        "default"
    }

    fn preview(&self) -> Option<ToolPreview> {
        match &self.state {
            State::ConnectingFromPort {
                from_world,
                pointer_world,
                ..
            } => Some(ToolPreview::GhostEdge {
                from_world: *from_world,
                to_world: *pointer_world,
                snap_target: None, // filled in by future enhancement
            }),
            State::RubberBand {
                origin_world,
                pointer_world,
                ..
            } => {
                let min = Pos::new(
                    origin_world.x.min(pointer_world.x),
                    origin_world.y.min(pointer_world.y),
                );
                let max = Pos::new(
                    origin_world.x.max(pointer_world.x),
                    origin_world.y.max(pointer_world.y),
                );
                Some(ToolPreview::RubberBand(Rect::from_min_max(min, max)))
            }
            _ => None,
        }
    }
}

impl DefaultTool {
    pub fn new() -> Self {
        Self::default()
    }

    fn on_primary_down(
        &mut self,
        world: Pos,
        extend: bool,
        toggle: bool,
        ops: &mut CanvasOps,
    ) {
        let landed_on = match ops.scene.hit_node(world, PORT_HIT_RADIUS) {
            Some((id, NodeHitKind::Port(port))) => {
                // Compute the port's world position for ghost-edge
                // origin so the ghost line starts at the port, not
                // the click point (which may be a few pixels off
                // from the actual port anchor).
                let port_world = ops
                    .scene
                    .node(id)
                    .and_then(|n| {
                        n.ports.iter().find(|p| p.id == port).map(|p| {
                            Pos::new(
                                n.rect.min.x + p.local_offset.x,
                                n.rect.min.y + p.local_offset.y,
                            )
                        })
                    })
                    .unwrap_or(world);
                PressTarget::Port(PortRef { node: id, port }, port_world)
            }
            Some((id, NodeHitKind::Body)) => PressTarget::NodeBody(id),
            None => {
                // Could still hit an edge — edges need a larger
                // click tolerance than ports.
                if let Some(eid) = ops.scene.hit_edge(world, 4.0) {
                    let item = SelectItem::Edge(eid);
                    self.apply_click_selection(item, extend, toggle, ops);
                    PressTarget::Empty
                } else {
                    PressTarget::Empty
                }
            }
        };
        self.state = State::PrimaryPressed {
            origin_world: world,
            landed_on,
            extend,
            toggle,
        };
    }

    fn on_pointer_move(&mut self, world: Pos, ops: &mut CanvasOps) {
        // Promote Pressed → Dragging once the pointer has moved past
        // the click threshold.
        if let State::PrimaryPressed {
            origin_world,
            landed_on,
            extend,
            toggle,
        } = &self.state
        {
            let dx = world.x - origin_world.x;
            let dy = world.y - origin_world.y;
            let moved_enough = (dx * dx + dy * dy).sqrt() > DRAG_THRESHOLD_WORLD;
            if !moved_enough {
                return;
            }
            let landed_on = landed_on.clone();
            let origin_world = *origin_world;
            let extend = *extend;
            let toggle = *toggle;
            match landed_on {
                PressTarget::NodeBody(id) => {
                    // If the clicked node wasn't already selected,
                    // replace the selection with just it before
                    // starting the drag — dragging an unselected
                    // node in Figma/Dymola implicitly selects it.
                    // (Selection is allowed in read-only mode — only
                    // scene mutations are blocked.)
                    if !ops.selection.contains(SelectItem::Node(id)) {
                        ops.selection.set(SelectItem::Node(id));
                        ops.events.push(SceneEvent::SelectionChanged(
                            ops.selection.clone(),
                        ));
                    }
                    // Read-only tab: refuse to enter the drag state.
                    // The user can still click to select, but any
                    // drag motion falls back to rubber-band selection
                    // below. Prevents authored scene mutation at the
                    // source, not via after-the-fact snap-back.
                    if ops.read_only {
                        self.state = State::Idle;
                        return;
                    }
                    // Snapshot current rects for every selected
                    // node. We apply translation from origin_world
                    // every frame, so we need the *pre-drag* rects
                    // to compute cumulative motion without drift.
                    let mut original = HashMap::new();
                    for nid in ops.selection.nodes() {
                        if let Some(n) = ops.scene.node(nid) {
                            original.insert(nid, n.rect);
                        }
                    }
                    self.state = State::DraggingNodes {
                        origin_world,
                        original_rects: original,
                    };
                }
                PressTarget::Port(from, from_world) => {
                    // Read-only tab: no new connections allowed.
                    if ops.read_only {
                        self.state = State::Idle;
                        return;
                    }
                    self.state = State::ConnectingFromPort {
                        from,
                        from_world,
                        pointer_world: world,
                    };
                }
                PressTarget::Empty => {
                    self.state = State::RubberBand {
                        origin_world,
                        pointer_world: world,
                        extend,
                        toggle,
                    };
                }
            }
            return;
        }

        // Feed in-flight drag state.
        match &mut self.state {
            State::DraggingNodes {
                origin_world,
                original_rects,
            } => {
                let mut dx = world.x - origin_world.x;
                let mut dy = world.y - origin_world.y;
                // Snap the *final position* of the drag anchor to the
                // grid, then derive a shared translation everyone
                // else inherits. Anchor = the node whose original
                // rect contains `origin_world` (the icon the user
                // clicked on); falls back to the first rect. Snapping
                // the translation instead of the final position was
                // wrong: node-at-(33.7,21.3) + snapped-dx-of-10 still
                // lands at (43.7,21.3), off-grid. Snapping the
                // anchor's final min makes the grabbed icon land
                // on a grid intersection; other multi-selected nodes
                // move by the same delta so their relative layout is
                // preserved.
                if let Some(snap) = ops.snap {
                    if snap.step > 0.0 {
                        let anchor = original_rects
                            .iter()
                            .find(|(_, r)| r.contains(*origin_world))
                            .or_else(|| original_rects.iter().next())
                            .map(|(_, r)| *r);
                        if let Some(anchor) = anchor {
                            let q = |v: f32| (v / snap.step).round() * snap.step;
                            let target_min_x = anchor.min.x + dx;
                            let target_min_y = anchor.min.y + dy;
                            dx = q(target_min_x) - anchor.min.x;
                            dy = q(target_min_y) - anchor.min.y;
                        }
                    }
                }
                for (nid, original) in original_rects.iter() {
                    if let Some(n) = ops.scene.node_mut(*nid) {
                        n.rect = original.translated(dx, dy);
                    }
                }
            }
            State::ConnectingFromPort {
                pointer_world, ..
            } => {
                *pointer_world = world;
            }
            State::RubberBand {
                pointer_world, ..
            } => {
                *pointer_world = world;
            }
            _ => {}
        }
    }

    fn on_primary_up(&mut self, world: Pos, ops: &mut CanvasOps) {
        // Take state by value so the match arms can't accidentally
        // leave us in a contradictory state.
        let state = std::mem::replace(&mut self.state, State::Idle);
        match state {
            State::PrimaryPressed {
                landed_on,
                extend,
                toggle,
                ..
            } => {
                // Click without drag — resolve as selection change.
                match landed_on {
                    PressTarget::NodeBody(id) => {
                        self.apply_click_selection(
                            SelectItem::Node(id),
                            extend,
                            toggle,
                            ops,
                        );
                    }
                    PressTarget::Port(_, _) => {
                        // A bare port-click with no drag does
                        // nothing — we don't select ports.
                    }
                    PressTarget::Empty => {
                        // Click on empty space — clear selection
                        // unless the user held shift/ctrl (in which
                        // case we don't deselect; this matches what
                        // Figma/Illustrator do).
                        if !extend && !toggle && !ops.selection.is_empty() {
                            ops.selection.clear();
                            ops.events.push(SceneEvent::SelectionChanged(
                                ops.selection.clone(),
                            ));
                        }
                    }
                }
            }

            State::DraggingNodes { original_rects, .. } => {
                // Emit one NodeMoved per moved node. Compare current
                // rect vs original; if unchanged (somehow) skip.
                for (nid, orig) in original_rects {
                    if let Some(n) = ops.scene.node(nid) {
                        if n.rect.min != orig.min {
                            ops.events.push(SceneEvent::NodeMoved {
                                id: nid,
                                old_min: orig.min,
                                new_min: n.rect.min,
                            });
                        }
                    }
                }
            }

            State::ConnectingFromPort { from, .. } => {
                // Commit the edge if the release landed on a
                // different node. First try a direct port hit;
                // if the release is on the body (between ports or
                // past them slightly), snap to that node's closest
                // port. Saves users from pixel-perfecting every
                // drop, matching Dymola / OMEdit behaviour.
                let target_node_and_port =
                    match ops.scene.hit_node(world, PORT_HIT_RADIUS) {
                        Some((nid, NodeHitKind::Port(pid))) => Some((nid, pid)),
                        Some((nid, NodeHitKind::Body)) => {
                            nearest_port_on_node(ops.scene, nid, world)
                                .map(|pid| (nid, pid))
                        }
                        None => None,
                    };
                if let Some((target_node, target_port)) = target_node_and_port {
                    if target_node != from.node {
                        let to = PortRef {
                            node: target_node,
                            port: target_port,
                        };
                        ops.events
                            .push(SceneEvent::EdgeCreated { from, to });
                    }
                }
                // Note: when release lands on pure empty space,
                // no edge is emitted. A future "dropped-wire menu"
                // (Snarl-style) can hook this path by emitting a
                // different SceneEvent — leave the slot open.
            }

            State::RubberBand {
                origin_world,
                pointer_world,
                extend,
                toggle,
            } => {
                let min = Pos::new(
                    origin_world.x.min(pointer_world.x),
                    origin_world.y.min(pointer_world.y),
                );
                let max = Pos::new(
                    origin_world.x.max(pointer_world.x),
                    origin_world.y.max(pointer_world.y),
                );
                let band = Rect::from_min_max(min, max);
                // "Any intersection with band" — including fully
                // contained nodes (Figma/Miro convention). A "fully
                // contained only" option is a UX switch we leave for
                // later.
                let hit_ids: Vec<NodeId> = ops
                    .scene
                    .nodes()
                    .filter_map(|(id, n)| {
                        if rects_intersect(n.rect, band) {
                            Some(*id)
                        } else {
                            None
                        }
                    })
                    .collect();
                if !extend && !toggle {
                    ops.selection.clear();
                }
                for id in hit_ids {
                    let it = SelectItem::Node(id);
                    if toggle {
                        ops.selection.toggle(it);
                    } else {
                        ops.selection.add(it);
                    }
                }
                ops.events
                    .push(SceneEvent::SelectionChanged(ops.selection.clone()));
            }

            State::Idle => {}
        }
    }

    /// Click-time selection resolution. Shared between body-click
    /// and edge-click paths.
    fn apply_click_selection(
        &mut self,
        item: SelectItem,
        extend: bool,
        toggle: bool,
        ops: &mut CanvasOps,
    ) {
        if toggle {
            ops.selection.toggle(item);
        } else if extend {
            ops.selection.add(item);
        } else {
            ops.selection.set(item);
        }
        ops.events
            .push(SceneEvent::SelectionChanged(ops.selection.clone()));
    }

    fn delete_selection(&mut self, ops: &mut CanvasOps) {
        if ops.selection.is_empty() {
            return;
        }
        // Edges first — deleting a node cascades to its edges, and
        // we want one event per doc operation. Collect ids up front
        // because removing mutates the IndexMap.
        let edge_ids: Vec<_> = ops.selection.edges().into_iter().collect();
        let node_ids: Vec<_> = ops.selection.nodes().into_iter().collect();
        for eid in edge_ids {
            if ops.scene.remove_edge(eid).is_some() {
                ops.events.push(SceneEvent::EdgeDeleted { id: eid });
            }
        }
        for nid in node_ids {
            if let Some((_n, orphans)) = ops.scene.remove_node(nid) {
                // Emit the orphan-edge events BEFORE the node
                // deletion so consumers that key on edge ids still
                // find the node in their mirror when they process
                // the edge event.
                for eid in &orphans {
                    ops.events.push(SceneEvent::EdgeDeleted { id: *eid });
                }
                ops.events.push(SceneEvent::NodeDeleted {
                    id: nid,
                    orphaned_edges: orphans,
                });
            }
        }
        ops.selection.clear();
        ops.events
            .push(SceneEvent::SelectionChanged(ops.selection.clone()));
    }
}

/// Find the port on `node_id` whose world-space anchor is closest
/// to `world_pos`. Used for "drop on body" snap-to-port on edge
/// creation. Returns `None` if the node has no ports.
fn nearest_port_on_node(
    scene: &Scene,
    node_id: NodeId,
    world_pos: Pos,
) -> Option<crate::scene::PortId> {
    let node = scene.node(node_id)?;
    let mut best: Option<(f32, crate::scene::PortId)> = None;
    for port in &node.ports {
        let px = node.rect.min.x + port.local_offset.x;
        let py = node.rect.min.y + port.local_offset.y;
        let dx = world_pos.x - px;
        let dy = world_pos.y - py;
        let d2 = dx * dx + dy * dy;
        if best.as_ref().map_or(true, |(bd, _)| d2 < *bd) {
            best = Some((d2, port.id.clone()));
        }
    }
    best.map(|(_, id)| id)
}

/// Axis-aligned rectangle intersection test (including touch).
fn rects_intersect(a: Rect, b: Rect) -> bool {
    a.min.x <= b.max.x && a.max.x >= b.min.x && a.min.y <= b.max.y && a.max.y >= b.min.y
}

// ─── tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Modifiers;
    use crate::scene::{Edge, EdgeId, Node, PortId, Port};

    fn mk_scene() -> Scene {
        let mut s = Scene::new();
        // A: (0,0)-(40,30), port "out" at right edge centre (40,15)
        s.insert_node(Node {
            id: NodeId(0),
            rect: Rect::from_min_size(Pos::new(0.0, 0.0), 40.0, 30.0),
            kind: "t".into(),
            data: serde_json::Value::Null,
            ports: vec![Port {
                id: PortId::new("out"),
                local_offset: Pos::new(40.0, 15.0),
                kind: "".into(),
            }],
            label: "A".into(),
            origin: None,
        });
        // B: (100,0)-(140,30), port "in" at left edge centre (0,15)
        s.insert_node(Node {
            id: NodeId(1),
            rect: Rect::from_min_size(Pos::new(100.0, 0.0), 40.0, 30.0),
            kind: "t".into(),
            data: serde_json::Value::Null,
            ports: vec![Port {
                id: PortId::new("in"),
                local_offset: Pos::new(0.0, 15.0),
                kind: "".into(),
            }],
            label: "B".into(),
            origin: None,
        });
        s
    }

    /// Helpers to synthesise input events quickly.
    fn down(world: Pos, shift: bool, ctrl: bool) -> InputEvent {
        InputEvent::PointerDown {
            button: MouseButton::Primary,
            world,
            screen: world,
            modifiers: Modifiers {
                shift,
                ctrl,
                alt: false,
            },
        }
    }
    fn mv(world: Pos) -> InputEvent {
        InputEvent::PointerMove {
            world,
            screen: world,
            modifiers: Modifiers::default(),
        }
    }
    fn up(world: Pos) -> InputEvent {
        InputEvent::PointerUp {
            button: MouseButton::Primary,
            world,
            screen: world,
            modifiers: Modifiers::default(),
        }
    }

    /// Spin up a fresh Tool + CanvasOps env for a test case.
    fn env() -> (DefaultTool, Scene, Selection, Viewport, Vec<SceneEvent>) {
        (
            DefaultTool::new(),
            mk_scene(),
            Selection::default(),
            Viewport::default(),
            Vec::new(),
        )
    }

    /// Run a sequence of events against the tool.
    fn run(
        tool: &mut DefaultTool,
        scene: &mut Scene,
        selection: &mut Selection,
        viewport: &mut Viewport,
        events: &mut Vec<SceneEvent>,
        seq: &[InputEvent],
    ) {
        for ev in seq {
            let mut ops = CanvasOps {
                scene,
                selection,
                viewport,
                events,
                read_only: false,
                snap: None,
            };
            tool.handle(ev, &mut ops);
        }
    }

    #[test]
    fn click_body_selects_node() {
        let (mut t, mut s, mut sel, mut vp, mut ev) = env();
        run(
            &mut t,
            &mut s,
            &mut sel,
            &mut vp,
            &mut ev,
            &[down(Pos::new(20.0, 15.0), false, false), up(Pos::new(20.0, 15.0))],
        );
        assert!(sel.contains(SelectItem::Node(NodeId(0))));
        assert_eq!(sel.len(), 1);
        // One SelectionChanged emitted.
        assert!(matches!(
            ev.last(),
            Some(SceneEvent::SelectionChanged(_))
        ));
    }

    #[test]
    fn click_empty_clears_selection() {
        let (mut t, mut s, mut sel, mut vp, mut ev) = env();
        sel.set(SelectItem::Node(NodeId(1))); // pre-populate
        run(
            &mut t,
            &mut s,
            &mut sel,
            &mut vp,
            &mut ev,
            &[down(Pos::new(500.0, 500.0), false, false), up(Pos::new(500.0, 500.0))],
        );
        assert!(sel.is_empty());
    }

    #[test]
    fn shift_click_extends_selection() {
        let (mut t, mut s, mut sel, mut vp, mut ev) = env();
        sel.set(SelectItem::Node(NodeId(0)));
        run(
            &mut t,
            &mut s,
            &mut sel,
            &mut vp,
            &mut ev,
            &[down(Pos::new(120.0, 15.0), true, false), up(Pos::new(120.0, 15.0))],
        );
        assert!(sel.contains(SelectItem::Node(NodeId(0))));
        assert!(sel.contains(SelectItem::Node(NodeId(1))));
    }

    #[test]
    fn press_with_no_move_is_a_click_not_a_drag() {
        // Moving less than DRAG_THRESHOLD must not promote to drag.
        let (mut t, mut s, mut sel, mut vp, mut ev) = env();
        run(
            &mut t,
            &mut s,
            &mut sel,
            &mut vp,
            &mut ev,
            &[
                down(Pos::new(20.0, 15.0), false, false),
                mv(Pos::new(20.5, 15.2)), // tiny wobble
                up(Pos::new(20.5, 15.2)),
            ],
        );
        // Nothing moved.
        assert_eq!(s.node(NodeId(0)).unwrap().rect.min, Pos::new(0.0, 0.0));
        // No NodeMoved emitted.
        assert!(!ev
            .iter()
            .any(|e| matches!(e, SceneEvent::NodeMoved { .. })));
        // But the click did select.
        assert!(sel.contains(SelectItem::Node(NodeId(0))));
    }

    #[test]
    fn drag_body_moves_node_and_emits_moved_on_release() {
        let (mut t, mut s, mut sel, mut vp, mut ev) = env();
        run(
            &mut t,
            &mut s,
            &mut sel,
            &mut vp,
            &mut ev,
            &[
                down(Pos::new(20.0, 15.0), false, false),
                mv(Pos::new(70.0, 45.0)), // +50, +30
                mv(Pos::new(80.0, 55.0)), // +60, +40
                up(Pos::new(80.0, 55.0)),
            ],
        );
        let n = s.node(NodeId(0)).unwrap();
        assert_eq!(n.rect.min, Pos::new(60.0, 40.0));
        let moved: Vec<_> = ev
            .iter()
            .filter_map(|e| match e {
                SceneEvent::NodeMoved {
                    id,
                    old_min,
                    new_min,
                } => Some((*id, *old_min, *new_min)),
                _ => None,
            })
            .collect();
        assert_eq!(moved.len(), 1);
        assert_eq!(moved[0], (NodeId(0), Pos::new(0.0, 0.0), Pos::new(60.0, 40.0)));
    }

    #[test]
    fn drag_from_port_to_port_creates_edge() {
        let (mut t, mut s, mut sel, mut vp, mut ev) = env();
        // Press on A's "out" port (world 40,15), drag to B's "in"
        // port (world 100,15), release.
        run(
            &mut t,
            &mut s,
            &mut sel,
            &mut vp,
            &mut ev,
            &[
                down(Pos::new(40.0, 15.0), false, false),
                mv(Pos::new(70.0, 15.0)), // en route
                mv(Pos::new(100.0, 15.0)),
                up(Pos::new(100.0, 15.0)),
            ],
        );
        let created: Vec<_> = ev
            .iter()
            .filter_map(|e| match e {
                SceneEvent::EdgeCreated { from, to } => Some((from.clone(), to.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].0.node, NodeId(0));
        assert_eq!(created[0].0.port, PortId::new("out"));
        assert_eq!(created[0].1.node, NodeId(1));
        assert_eq!(created[0].1.port, PortId::new("in"));
    }

    #[test]
    fn drag_from_port_to_empty_does_not_create_edge() {
        let (mut t, mut s, mut sel, mut vp, mut ev) = env();
        run(
            &mut t,
            &mut s,
            &mut sel,
            &mut vp,
            &mut ev,
            &[
                down(Pos::new(40.0, 15.0), false, false),
                mv(Pos::new(200.0, 200.0)),
                up(Pos::new(200.0, 200.0)),
            ],
        );
        assert!(!ev
            .iter()
            .any(|e| matches!(e, SceneEvent::EdgeCreated { .. })));
    }

    #[test]
    fn delete_key_removes_selected_node_and_orphan_edges() {
        let (mut t, mut s, mut sel, mut vp, mut ev) = env();
        // Add an edge A→B so we can observe orphan cleanup.
        s.insert_edge(Edge {
            id: EdgeId(42),
            from: PortRef {
                node: NodeId(0),
                port: PortId::new("out"),
            },
            to: PortRef {
                node: NodeId(1),
                port: PortId::new("in"),
            },
            kind: "t".into(),
            data: serde_json::Value::Null,
            origin: None,
        });
        sel.set(SelectItem::Node(NodeId(0)));
        run(
            &mut t,
            &mut s,
            &mut sel,
            &mut vp,
            &mut ev,
            &[InputEvent::Key {
                name: "Delete",
                modifiers: Modifiers::default(),
            }],
        );
        assert!(s.node(NodeId(0)).is_none());
        assert!(s.edge(EdgeId(42)).is_none());
        // Events emitted in the right order: orphan EdgeDeleted
        // before NodeDeleted.
        let kinds: Vec<_> = ev
            .iter()
            .filter_map(|e| match e {
                SceneEvent::EdgeDeleted { id } => Some(("edge", id.0)),
                SceneEvent::NodeDeleted { id, .. } => Some(("node", id.0)),
                _ => None,
            })
            .collect();
        assert_eq!(kinds, vec![("edge", 42), ("node", 0)]);
    }

    #[test]
    fn rubber_band_selects_enclosed_nodes() {
        let (mut t, mut s, mut sel, mut vp, mut ev) = env();
        run(
            &mut t,
            &mut s,
            &mut sel,
            &mut vp,
            &mut ev,
            &[
                // Press in empty space (between A at x=40 and B at x=100).
                down(Pos::new(60.0, -10.0), false, false),
                mv(Pos::new(60.5, -9.5)), // exit "pressed", enter rubber-band
                mv(Pos::new(150.0, 40.0)), // cover B only
                up(Pos::new(150.0, 40.0)),
            ],
        );
        assert!(!sel.contains(SelectItem::Node(NodeId(0))));
        assert!(sel.contains(SelectItem::Node(NodeId(1))));
    }

    #[test]
    fn escape_aborts_drag_release_does_not_emit_move() {
        // Escape mid-drag cancels the interaction. Selection
        // still reflects the press (clicking a non-selected body
        // implicitly selects it — Figma/Dymola behaviour), but
        // no NodeMoved event fires on the subsequent mouse-up
        // because we're back in Idle by then.
        //
        // We do NOT currently revert the node's in-flight position
        // to its original — that's a nice-to-have (Figma does it,
        // Dymola doesn't). If users ask, the `original_rects` is
        // still in the state enum just before Escape clears it,
        // so the revert is a ~5-line addition.
        let (mut t, mut s, mut sel, mut vp, mut ev) = env();
        run(
            &mut t,
            &mut s,
            &mut sel,
            &mut vp,
            &mut ev,
            &[
                down(Pos::new(20.0, 15.0), false, false), // A body
                mv(Pos::new(70.0, 15.0)),                 // begin drag
                InputEvent::Key {
                    name: "Escape",
                    modifiers: Modifiers::default(),
                },
                up(Pos::new(70.0, 15.0)),
            ],
        );
        // No NodeMoved — the release after Escape hits the Idle
        // branch and emits nothing.
        assert!(!ev
            .iter()
            .any(|e| matches!(e, SceneEvent::NodeMoved { .. })));
        // Selection is whatever the press set it to: Node(0),
        // because clicking an unselected body replaces selection.
        assert!(sel.contains(SelectItem::Node(NodeId(0))));
    }

    #[test]
    fn double_click_on_body_emits_drill_in_event() {
        let (mut t, mut s, mut sel, mut vp, mut ev) = env();
        run(
            &mut t,
            &mut s,
            &mut sel,
            &mut vp,
            &mut ev,
            &[InputEvent::DoubleClick {
                world: Pos::new(20.0, 15.0),
                screen: Pos::new(20.0, 15.0),
                modifiers: Modifiers::default(),
            }],
        );
        let found = ev
            .iter()
            .any(|e| matches!(e, SceneEvent::NodeDoubleClicked { id } if *id == NodeId(0)));
        assert!(found);
    }
}
