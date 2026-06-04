//! Interaction trait — what turns mouse/keyboard events into
//! scene mutations.
//!
//! Exactly one tool is **active** at a time; the canvas dispatches
//! each [`InputEvent`] to the active tool,
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
//! and emit [`SceneEvent`]s, but not reach
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

use crate::event::{InputEvent, MouseButton, SceneEvent};
use crate::scene::{EdgeId, NodeHitKind, NodeId, PortRef, Pos, Rect, Scene};
use crate::selection::{SelectItem, Selection};
use crate::visual::EdgeHit;
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
    /// authored scene). Surfaced as a [`crate::Canvas::read_only`] field
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

    /// Whether any non-idle gesture is in flight — drag, resize,
    /// rubber-band, port-connect. Canvas uses this for stale-drag
    /// rescue: if egui reports the mouse button as NOT held but the
    /// tool is still mid-gesture (release fired outside the OS
    /// window, focus loss, missed event), synthesise a release so
    /// the gesture finalises and selection chrome can clear.
    /// Default: false (idle), suitable for stateless tools.
    fn is_active(&self) -> bool {
        false
    }

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
    /// Same as [`Self::GhostEdge`] but with intermediate bend points
    /// the user has placed via click-to-bend. Layers render the
    /// polyline `from_world → bends[0] → … → bends[N-1] → to_world`.
    GhostEdgeWithBends {
        from_world: Pos,
        bends: Vec<Pos>,
        to_world: Pos,
        snap_target: Option<Pos>,
    },
    /// Rubber-band selection rectangle, world-space.
    RubberBand(Rect),
    /// Alignment guide lines drawn while a waypoint snaps to another
    /// wire's bend coordinate. `x`/`y` are world-space lines; either
    /// can be `None` (only the matching axis drew a guide).
    SnapGuides {
        x: Option<f32>,
        y: Option<f32>,
    },
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
    ///
    /// `points` is the list of bends the user placed mid-creation by
    /// clicking in empty space (Dymola/OMEdit-style click-to-bend).
    /// `mode` distinguishes the two ways to enter this state — `Drag`
    /// commits on release (the original press-and-drag flow), while
    /// `Click` only commits on a port-click (the new click-to-bend
    /// flow entered via a bare port-click).
    ConnectingFromPort {
        from: PortRef,
        from_world: Pos,
        pointer_world: Pos,
        points: Vec<Pos>,
        mode: ConnectMode,
    },

    /// Resizing a node by its bottom-right handle. Mutates
    /// `scene.nodes[id].rect.max` live each frame; emits a
    /// `NodeResized` event on release. Plot / control / dashboard
    /// nodes use this as their primary resize affordance.
    ResizingNode {
        id: NodeId,
        original_rect: Rect,
        origin_world: Pos,
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

    /// Dragging a corner or segment handle of an edge's polyline.
    /// `original` is the interior-waypoints list at press time —
    /// preserved so Escape can revert and the on-up event carries the
    /// pre-drag → post-drag delta cleanly. `current` is mutated each
    /// PointerMove and mirrored into `scene.edge_mut(edge).waypoints`
    /// so the visual reflects the drag without per-frame events.
    DraggingEdgeWaypoint {
        edge: EdgeId,
        hit: EdgeHit,
        origin_world: Pos,
        /// Edge endpoint positions captured at press time — used so
        /// segment-drag math stays consistent even if a parent node
        /// jitters during the drag.
        from_world: Pos,
        to_world: Pos,
        original: Vec<Pos>,
        current: Vec<Pos>,
        /// Most recent snap-to-other-wire alignment hit, in world
        /// coords — `(Some(x), _)` means we snapped to vertical line
        /// `x`. Renderer reads this to draw guide lines.
        snap_x: Option<f32>,
        snap_y: Option<f32>,
    },
}

/// How a wire-creation gesture was entered. The two modes have
/// different release semantics — see [`State::ConnectingFromPort`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectMode {
    /// Entered via press-and-drag from a port. Release on a port
    /// commits; release on empty space cancels (legacy behaviour).
    Drag,
    /// Entered via a bare port click (press + release on the same
    /// port without moving). The wire follows the cursor; subsequent
    /// clicks in empty space place bends; a click on a target port
    /// commits.
    Click,
}

/// Which scene element the primary press landed on — recorded at
/// press-time so the drag promotion knows what to do.
#[derive(Debug, Clone)]
enum PressTarget {
    /// Bottom-right resize handle of a node — promotes to
    /// `ResizingNode` on drag.
    ResizeHandle(NodeId),
    NodeBody(NodeId),
    Port(PortRef, Pos), // port world-space position for ghost edge origin
    /// On an edge's waypoint or segment handle. Drag promotes to
    /// `DraggingEdgeWaypoint`.
    EdgeHandle(EdgeId, EdgeHit),
    /// On an edge's body (not a handle). Click selects the edge on
    /// release; drag falls back to rubber-band (the body itself isn't
    /// draggable — Dymola convention).
    EdgeBody(EdgeId),
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

    fn is_active(&self) -> bool {
        !matches!(self.state, State::Idle)
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
                    PressTarget::ResizeHandle(id) => {
                        if let Some(new_id) = find_new_id(*id) {
                            *id = new_id;
                        } else {
                            *landed_on = PressTarget::Empty;
                        }
                    }
                    PressTarget::EdgeBody(_) => {
                        // EdgeIds aren't remapped by this helper.
                    }
                    PressTarget::EdgeHandle(_, _) => {
                        // EdgeIds aren't remapped by this helper —
                        // the caller only deals with NodeId remapping
                        // after a re-projection. Drop the press
                        // target to be safe; the user will need to
                        // re-press if the projection landed mid-press.
                        *landed_on = PressTarget::Empty;
                    }
                }
            }
            State::ResizingNode { id, .. } => {
                if let Some(new_id) = find_new_id(*id) {
                    *id = new_id;
                } else {
                    self.state = State::Idle;
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
            State::ConnectingFromPort { from, points: _, .. } => {
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
                // Special-case: when wire-drawing is already active
                // (entered via drag-promote or bare-port-click), a
                // press on a port commits, a press in empty space
                // appends a bend, and a press on the source port
                // cancels. Don't fall through to `on_primary_down`,
                // which would reset state to PrimaryPressed.
                if matches!(self.state, State::ConnectingFromPort { .. }) {
                    self.on_wire_drawing_click(*world, ops);
                    return ToolOutcome::Consumed;
                }
                self.on_primary_down(*world, modifiers.shift, modifiers.ctrl, ops);
                ToolOutcome::Consumed
            }

            InputEvent::PointerMove { world, modifiers, .. } => {
                self.last_pointer_world = Some(*world);
                // A move might promote Pressed → Dragging, or feed
                // an in-flight drag. Pan (middle-drag) lives in
                // navigation so we don't consume when idle.
                if matches!(self.state, State::Idle) {
                    ToolOutcome::Passthrough
                } else {
                    self.on_pointer_move(*world, *modifiers, ops);
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
                    // Abort any in-flight drag/connect. Revert
                    // mid-drag mutations so the scene looks like the
                    // user never started the gesture (Figma-style).
                    let state = std::mem::replace(&mut self.state, State::Idle);
                    match state {
                        State::DraggingNodes {
                            original_rects, ..
                        } => {
                            for (nid, orig) in original_rects {
                                if let Some(n) = ops.scene.node_mut(nid) {
                                    n.rect = orig;
                                }
                            }
                        }
                        State::ResizingNode {
                            id, original_rect, ..
                        } => {
                            if let Some(n) = ops.scene.node_mut(id) {
                                n.rect = original_rect;
                            }
                        }
                        State::DraggingEdgeWaypoint {
                            edge, original, ..
                        } => {
                            if let Some(e) = ops.scene.edge_mut(edge) {
                                e.waypoints = original;
                            }
                        }
                        _ => {}
                    }
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
                points,
                ..
            } => Some(ToolPreview::GhostEdgeWithBends {
                from_world: *from_world,
                bends: points.clone(),
                to_world: *pointer_world,
                snap_target: None,
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
            State::DraggingEdgeWaypoint { snap_x, snap_y, .. }
                if snap_x.is_some() || snap_y.is_some() =>
            {
                Some(ToolPreview::SnapGuides {
                    x: *snap_x,
                    y: *snap_y,
                })
            }
            _ => None,
        }
    }
}

impl DefaultTool {
    pub fn new() -> Self {
        Self::default()
    }

    /// Handle a primary click while already in `ConnectingFromPort`
    /// state — implements Dymola/OMEdit click-to-bend during wire
    /// creation. Click on a different node's port = commit; click
    /// on the source port or empty space inside the source node =
    /// cancel; click in empty world space = append a bend.
    fn on_wire_drawing_click(&mut self, world: Pos, ops: &mut CanvasOps) {
        let State::ConnectingFromPort { from, from_world, points, mode, .. } =
            std::mem::replace(&mut self.state, State::Idle)
        else {
            return;
        };
        // Hit test: port → commit / cancel; body → ignore (don't
        // commit through a node body that isn't a port); empty → bend.
        let target = ops.scene.hit_node(world, PORT_HIT_RADIUS);
        match target {
            Some((nid, NodeHitKind::Port(pid))) => {
                if nid == from.node && pid == from.port {
                    // Clicked the source port again — cancel.
                    return;
                }
                if nid != from.node {
                    let to = PortRef { node: nid, port: pid };
                    ops.events.push(SceneEvent::EdgeCreated {
                        from,
                        to,
                        points,
                    });
                    return;
                }
                // Same-node port — keep drawing.
                self.state = State::ConnectingFromPort {
                    from,
                    from_world,
                    pointer_world: world,
                    points,
                    mode,
                };
            }
            Some((_, NodeHitKind::Body)) => {
                // Clicking on a node body without hitting a port
                // doesn't commit (Dymola requires a port hit) — stay
                // in wire-drawing mode.
                self.state = State::ConnectingFromPort {
                    from,
                    from_world,
                    pointer_world: world,
                    points,
                    mode,
                };
            }
            None => {
                // Empty space — append a bend at the click position
                // (grid-snap so the bend lands cleanly).
                let (sx, sy) = snap_point(world.x, world.y, ops.snap);
                let mut new_points = points;
                new_points.push(Pos::new(sx, sy));
                self.state = State::ConnectingFromPort {
                    from,
                    from_world,
                    pointer_world: world,
                    points: new_points,
                    mode,
                };
            }
        }
    }

    fn on_primary_down(
        &mut self,
        world: Pos,
        extend: bool,
        toggle: bool,
        ops: &mut CanvasOps,
    ) {
        // Resize-handle hit test takes priority over node-body —
        // press inside the bottom-right ~6 world-unit corner of any
        // node enters resize mode. Skipped on read-only tabs (the
        // promote-to-Resizing branch in `on_pointer_move` also
        // honours this, but bailing here saves the state churn).
        if !ops.read_only {
            const RESIZE_HANDLE_RADIUS: f32 = 9.0;
            let mut handle_hit: Option<NodeId> = None;
            for (nid, node) in ops.scene.nodes() {
                if !node.resizable {
                    continue;
                }
                let dx = world.x - node.rect.max.x;
                let dy = world.y - node.rect.max.y;
                if dx * dx + dy * dy
                    <= RESIZE_HANDLE_RADIUS * RESIZE_HANDLE_RADIUS
                {
                    handle_hit = Some(*nid);
                    break;
                }
            }
            if let Some(id) = handle_hit {
                self.state = State::PrimaryPressed {
                    origin_world: world,
                    landed_on: PressTarget::ResizeHandle(id),
                    extend,
                    toggle,
                };
                return;
            }
        }
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
                // Could still hit an edge. Tolerances are quoted in
                // *screen points* and scaled to world units by the
                // current zoom — otherwise 5-world-unit handles shrink
                // off-grabbable at high zoom and balloon at low zoom.
                const EDGE_BODY_TOL_PX: f32 = 4.0;
                const EDGE_HANDLE_RADIUS_PX: f32 = 5.0;
                let zoom = ops.viewport.zoom.max(0.001);
                let body_tol = EDGE_BODY_TOL_PX / zoom;
                let handle_r = EDGE_HANDLE_RADIUS_PX / zoom;
                let handle_r_sq = handle_r * handle_r;
                // Hit-test handles on a single edge. Returns the first
                // matching corner or segment-midpoint, if any.
                let test_handles = |edge: &crate::scene::Edge,
                                    from: Pos,
                                    to: Pos|
                 -> Option<EdgeHit> {
                    for (i, w) in edge.waypoints.iter().enumerate() {
                        let dx = world.x - w.x;
                        let dy = world.y - w.y;
                        if dx * dx + dy * dy <= handle_r_sq {
                            return Some(EdgeHit::Corner(i));
                        }
                    }
                    let mut pts: Vec<Pos> =
                        Vec::with_capacity(2 + edge.waypoints.len());
                    pts.push(from);
                    pts.extend(edge.waypoints.iter().copied());
                    pts.push(to);
                    for i in 0..pts.len() - 1 {
                        let sx = pts[i + 1].x - pts[i].x;
                        let sy = pts[i + 1].y - pts[i].y;
                        // Skip segments shorter than the handle diameter
                        // to avoid overlap with corner handles.
                        if sx * sx + sy * sy <= 4.0 * handle_r_sq {
                            continue;
                        }
                        let mx = (pts[i].x + pts[i + 1].x) * 0.5;
                        let my = (pts[i].y + pts[i + 1].y) * 0.5;
                        let dx = world.x - mx;
                        let dy = world.y - my;
                        if dx * dx + dy * dy <= handle_r_sq {
                            return Some(EdgeHit::Segment(i));
                        }
                    }
                    None
                };
                let mut press_target = PressTarget::Empty;
                let mut handle_hit: Option<(EdgeId, EdgeHit)> = None;
                // Prefer handles on already-selected edges (their
                // handles render on top), then fall through to the
                // edge currently under the cursor — so a single press
                // on a corner/midpoint of an unselected wire starts
                // the drag without a prior select step.
                for eid in ops.selection.edges() {
                    let Some(edge) = ops.scene.edge(eid) else { continue };
                    let Some((from, to)) = ops.scene.edge_endpoint_positions(edge) else { continue };
                    if let Some(hit) = test_handles(edge, from, to) {
                        handle_hit = Some((eid, hit));
                        break;
                    }
                }
                if handle_hit.is_none() {
                    if let Some(eid) = ops.scene.hit_edge(world, body_tol) {
                        if !ops.selection.contains(SelectItem::Edge(eid)) {
                            if let Some(edge) = ops.scene.edge(eid) {
                                if let Some((from, to)) =
                                    ops.scene.edge_endpoint_positions(edge)
                                {
                                    if let Some(hit) =
                                        test_handles(edge, from, to)
                                    {
                                        handle_hit = Some((eid, hit));
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some((eid, hit)) = handle_hit {
                    if !ops.read_only {
                        press_target = PressTarget::EdgeHandle(eid, hit);
                    } else {
                        press_target = PressTarget::EdgeBody(eid);
                    }
                } else if let Some(eid) = ops.scene.hit_edge(world, body_tol) {
                    press_target = PressTarget::EdgeBody(eid);
                }
                press_target
            }
        };
        self.state = State::PrimaryPressed {
            origin_world: world,
            landed_on,
            extend,
            toggle,
        };
    }

    fn on_pointer_move(
        &mut self,
        world: Pos,
        modifiers: crate::event::Modifiers,
        ops: &mut CanvasOps,
    ) {
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
                        points: Vec::new(),
                        mode: ConnectMode::Drag,
                    };
                }
                PressTarget::Empty | PressTarget::EdgeBody(_) => {
                    self.state = State::RubberBand {
                        origin_world,
                        pointer_world: world,
                        extend,
                        toggle,
                    };
                }
                PressTarget::ResizeHandle(id) => {
                    if ops.read_only {
                        self.state = State::Idle;
                        return;
                    }
                    let original_rect = match ops.scene.node(id) {
                        Some(n) => n.rect,
                        None => {
                            self.state = State::Idle;
                            return;
                        }
                    };
                    self.state = State::ResizingNode {
                        id,
                        original_rect,
                        origin_world,
                    };
                }
                PressTarget::EdgeHandle(edge, hit) => {
                    if ops.read_only {
                        self.state = State::Idle;
                        return;
                    }
                    let (from_world, to_world, original) =
                        match ops.scene.edge(edge) {
                            Some(e) => {
                                match ops.scene.edge_endpoint_positions(e) {
                                    Some((f, t)) => (f, t, e.waypoints.clone()),
                                    None => {
                                        self.state = State::Idle;
                                        return;
                                    }
                                }
                            }
                            None => {
                                self.state = State::Idle;
                                return;
                            }
                        };
                    self.state = State::DraggingEdgeWaypoint {
                        edge,
                        hit,
                        origin_world,
                        from_world,
                        to_world,
                        original: original.clone(),
                        current: original,
                        snap_x: None,
                        snap_y: None,
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
            State::ResizingNode {
                id,
                original_rect,
                origin_world,
            } => {
                let dx = world.x - origin_world.x;
                let dy = world.y - origin_world.y;
                let new_max_x = (original_rect.max.x + dx)
                    .max(original_rect.min.x + 8.0);
                let new_max_y = (original_rect.max.y + dy)
                    .max(original_rect.min.y + 8.0);
                if let Some(n) = ops.scene.node_mut(*id) {
                    n.rect = Rect::from_min_max(
                        original_rect.min,
                        Pos::new(new_max_x, new_max_y),
                    );
                }
            }
            State::DraggingEdgeWaypoint {
                edge,
                hit,
                origin_world,
                from_world,
                to_world,
                original,
                current,
                snap_x,
                snap_y,
            } => {
                *snap_x = None;
                *snap_y = None;
                let mut dx = world.x - origin_world.x;
                let mut dy = world.y - origin_world.y;
                // Shift = constrain to the dominant axis (locks to
                // pure horizontal or vertical motion). Useful for
                // keeping a wire orthogonal while dragging a corner.
                if modifiers.shift {
                    if dx.abs() >= dy.abs() {
                        dy = 0.0;
                    } else {
                        dx = 0.0;
                    }
                }
                // Alt bypasses grid snap (Dymola convention).
                let active_snap = if modifiers.alt { None } else { ops.snap };
                // Recompute `current` from `original + delta` each
                // frame so the math is drift-free even after many
                // PointerMoves.
                let mut new_pts = original.clone();
                match hit {
                    EdgeHit::Corner(i) => {
                        if let Some(p) = new_pts.get_mut(*i) {
                            let (mut sx, mut sy) =
                                snap_point(p.x + dx, p.y + dy, active_snap);
                            // Wire-alignment snap: when nearby (within
                            // tolerance) bend coords on other wires
                            // share x/y, lock to them. Cheap O(W·B)
                            // scan; W and B are small at typical
                            // schematic complexity. Alt bypasses
                            // (mirrors the grid-snap bypass).
                            if !modifiers.alt {
                                const ALIGN_TOL: f32 = 3.0;
                                let mut best_dx: Option<(f32, f32)> = None;
                                let mut best_dy: Option<(f32, f32)> = None;
                                for (eid, other) in ops.scene.edges() {
                                    if *eid == *edge { continue; }
                                    for w in &other.waypoints {
                                        let dx_to = (w.x - sx).abs();
                                        if dx_to < ALIGN_TOL
                                            && best_dx.map_or(true, |(d, _)| dx_to < d)
                                        {
                                            best_dx = Some((dx_to, w.x));
                                        }
                                        let dy_to = (w.y - sy).abs();
                                        if dy_to < ALIGN_TOL
                                            && best_dy.map_or(true, |(d, _)| dy_to < d)
                                        {
                                            best_dy = Some((dy_to, w.y));
                                        }
                                    }
                                }
                                if let Some((_, x)) = best_dx {
                                    sx = x;
                                    *snap_x = Some(x);
                                }
                                if let Some((_, y)) = best_dy {
                                    sy = y;
                                    *snap_y = Some(y);
                                }
                            }
                            *p = Pos::new(sx, sy);
                        }
                    }
                    EdgeHit::Segment(i) => {
                        // Build the full polyline (port → interior →
                        // port) so segment endpoints are addressable
                        // uniformly. Segment i spans indices i..i+1.
                        let n = new_pts.len();
                        let pre = if *i == 0 { *from_world } else { new_pts[*i - 1] };
                        let post = if *i + 1 > n {
                            *to_world
                        } else if *i == n {
                            *to_world
                        } else {
                            new_pts[*i]
                        };
                        // Dymola: slide the segment perpendicular to
                        // its own axis. If the segment is horizontal
                        // (|sx| > |sy|), apply Δy to both endpoints;
                        // if vertical, apply Δx. The endpoints of the
                        // interior polyline that belong to this
                        // segment must be inserted if they're the
                        // port endpoints (i == 0 or i == n).
                        let sx = post.x - pre.x;
                        let sy = post.y - pre.y;
                        let perpendicular_dy =
                            sx.abs() >= sy.abs();
                        let (qdx, qdy) = if perpendicular_dy {
                            (0.0_f32, dy)
                        } else {
                            (dx, 0.0_f32)
                        };
                        // Move whichever endpoints of segment i are
                        // interior bends.
                        if *i >= 1 && *i - 1 < new_pts.len() {
                            let p = new_pts[*i - 1];
                            let (nx, ny) =
                                snap_point(p.x + qdx, p.y + qdy, active_snap);
                            new_pts[*i - 1] = Pos::new(nx, ny);
                        } else {
                            // Segment 0 starts at the from-port — we
                            // can't move the port, so insert a fresh
                            // bend right next to it that captures the
                            // perpendicular slide. Subsequent frames
                            // keep nudging that same bend.
                            let (nx, ny) = snap_point(
                                from_world.x + qdx,
                                from_world.y + qdy,
                                active_snap,
                            );
                            new_pts.insert(0, Pos::new(nx, ny));
                        }
                        if *i < new_pts.len() {
                            let p = new_pts[*i];
                            let (nx, ny) =
                                snap_point(p.x + qdx, p.y + qdy, active_snap);
                            new_pts[*i] = Pos::new(nx, ny);
                        } else {
                            // Last segment ends at the to-port — same
                            // trick: insert a bend at the port end.
                            let (nx, ny) = snap_point(
                                to_world.x + qdx,
                                to_world.y + qdy,
                                active_snap,
                            );
                            new_pts.push(Pos::new(nx, ny));
                        }
                    }
                    EdgeHit::Body => { /* shouldn't reach here */ }
                }
                *current = new_pts.clone();
                if let Some(e) = ops.scene.edge_mut(*edge) {
                    e.waypoints = new_pts;
                }
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
                    PressTarget::Port(from, from_world) => {
                        // Bare port-click → enter click-to-bend wire
                        // mode. Subsequent clicks in empty space
                        // append bends; a click on a target port
                        // commits the wire. Esc cancels. Read-only
                        // tabs bail (mirrors the drag path).
                        if ops.read_only {
                            return;
                        }
                        self.state = State::ConnectingFromPort {
                            from,
                            from_world,
                            pointer_world: from_world,
                            points: Vec::new(),
                            mode: ConnectMode::Click,
                        };
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
                    PressTarget::ResizeHandle(id) => {
                        // Bare click on a handle (no drag) — treat as
                        // a select on the node so the user gets visual
                        // confirmation they hit it.
                        self.apply_click_selection(
                            SelectItem::Node(id),
                            extend,
                            toggle,
                            ops,
                        );
                    }
                    PressTarget::EdgeBody(eid) => {
                        self.apply_click_selection(
                            SelectItem::Edge(eid),
                            extend,
                            toggle,
                            ops,
                        );
                    }
                    PressTarget::EdgeHandle(_, _) => {
                        // Bare click on a waypoint/segment handle with
                        // no drag: keep the edge selected, no other
                        // mutation. (A right-click context menu on the
                        // handle is the future home of "Delete bend".)
                    }
                }
            }

            State::ResizingNode { id, original_rect, .. } => {
                // Live mutation already happened during the drag.
                // Emit a single `NodeResized` on release so domain
                // code can persist the new size (resizable component
                // icons translate it to a `SetPlacement` with the
                // new width/height). Skip
                // if the rect didn't actually change — same noise
                // suppression `NodeMoved` does.
                if let Some(n) = ops.scene.node(id) {
                    if n.rect != original_rect {
                        ops.events.push(SceneEvent::NodeResized {
                            id,
                            old_rect: original_rect,
                            new_rect: n.rect,
                        });
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

            State::ConnectingFromPort { from, points, mode, .. } => {
                // `Click` mode releases (between click-to-bend
                // events) are not commit signals — they're just the
                // pointer button coming back up after each individual
                // click. Restore the state and bail. Only an
                // explicit port-click during a PointerDown commits
                // a Click-mode wire (handled in `on_primary_down`).
                if mode == ConnectMode::Click {
                    self.state = State::ConnectingFromPort {
                        from,
                        from_world: self
                            .last_pointer_world
                            .unwrap_or(world),
                        pointer_world: world,
                        points,
                        mode,
                    };
                    return;
                }
                // Drag mode: commit on release-over-a-port (snap to
                // body if close enough). Empty-space release cancels.
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
                        ops.events.push(SceneEvent::EdgeCreated {
                            from,
                            to,
                            points,
                        });
                    }
                }
                // Note: when release lands on pure empty space,
                // no edge is emitted. A future "dropped-wire menu"
                // (Snarl-style) can hook this path by emitting a
                // different SceneEvent — leave the slot open.
            }

            State::DraggingEdgeWaypoint {
                edge,
                original,
                current,
                ..
            } => {
                // Collinear cleanup: drop interior points P[i] where
                // P[i-1], P[i], P[i+1] are collinear within eps. Lets
                // segment-drag-then-back-to-line restore the cleaner
                // shape instead of leaving a redundant kink. Mirror
                // the cleaned polyline back into the scene so the
                // next render reflects it, then emit the change for
                // the host to translate into a domain op.
                let cleaned = cleanup_collinear(&current);
                if let Some(e) = ops.scene.edge_mut(edge) {
                    e.waypoints = cleaned.clone();
                    // First edit on an auto-routed wire captures its
                    // path into source — flip the authored flag so
                    // future rubber-band passes treat the wire as
                    // user-authored.
                    e.waypoints_authored = true;
                }
                if cleaned != original {
                    ops.events.push(SceneEvent::EdgeWaypointsChanged {
                        id: edge,
                        points: cleaned,
                    });
                }
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
                // Also collect edges whose polyline (endpoints +
                // interior waypoints) intersects the band — wires can
                // now be box-selected for batch delete / re-style.
                let hit_edges: Vec<EdgeId> = ops
                    .scene
                    .edges()
                    .filter_map(|(eid, e)| {
                        let Some((from, to)) =
                            ops.scene.edge_endpoint_positions(e)
                        else {
                            return None;
                        };
                        let any_in = |p: Pos| -> bool {
                            band.contains(p)
                        };
                        if any_in(from)
                            || any_in(to)
                            || e.waypoints.iter().any(|w| any_in(*w))
                        {
                            return Some(*eid);
                        }
                        // Final fallback: any segment crosses the band.
                        let mut pts: Vec<Pos> =
                            Vec::with_capacity(2 + e.waypoints.len());
                        pts.push(from);
                        pts.extend(e.waypoints.iter().copied());
                        pts.push(to);
                        for w in pts.windows(2) {
                            if segment_rect_intersects(w[0], w[1], band) {
                                return Some(*eid);
                            }
                        }
                        None
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
                for id in hit_edges {
                    let it = SelectItem::Edge(id);
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

/// True when the segment `(a, b)` crosses or lies inside the
/// axis-aligned rectangle `r`. Used so a rubber-band that doesn't
/// enclose either wire endpoint but crosses a long segment still
/// selects the wire.
fn segment_rect_intersects(a: Pos, b: Pos, r: Rect) -> bool {
    // Trivial accept: either endpoint inside.
    if r.contains(a) || r.contains(b) {
        return true;
    }
    // Clip the segment to the rect on each axis using the Liang-
    // Barsky parametric approach. If the resulting t-interval is
    // non-empty within [0, 1], the segment intersects the rect.
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let mut t_enter: f32 = 0.0;
    let mut t_exit: f32 = 1.0;
    let clip = |p: f32, q: f32, t_enter: &mut f32, t_exit: &mut f32| -> bool {
        if p.abs() < f32::EPSILON {
            // Parallel to clip edge: accept iff inside.
            return q >= 0.0;
        }
        let t = q / p;
        if p < 0.0 {
            if t > *t_exit { return false; }
            if t > *t_enter { *t_enter = t; }
        } else {
            if t < *t_enter { return false; }
            if t < *t_exit { *t_exit = t; }
        }
        true
    };
    clip(-dx, a.x - r.min.x, &mut t_enter, &mut t_exit)
        && clip(dx, r.max.x - a.x, &mut t_enter, &mut t_exit)
        && clip(-dy, a.y - r.min.y, &mut t_enter, &mut t_exit)
        && clip(dy, r.max.y - a.y, &mut t_enter, &mut t_exit)
}

/// Quantise `(x, y)` to the canvas grid when snap is enabled. No-op
/// when snap is `None` or its step is non-positive. Mirrors what the
/// node-drag path does so waypoint drags land on the same grid as
/// node placements.
fn snap_point(x: f32, y: f32, snap: Option<crate::canvas::SnapSettings>) -> (f32, f32) {
    if let Some(s) = snap {
        if s.step > 0.0 {
            let q = |v: f32| (v / s.step).round() * s.step;
            return (q(x), q(y));
        }
    }
    (x, y)
}

/// Drop interior points that are collinear with their neighbours
/// (within `eps`). Lets the waypoint editor return to a cleaner shape
/// when the user drags a corner back onto its neighbours' line. Port
/// endpoints are not part of `pts` (interior-only invariant of
/// [`crate::scene::Edge::waypoints`]).
fn cleanup_collinear(pts: &[Pos]) -> Vec<Pos> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    const EPS: f32 = 0.5;
    let mut out: Vec<Pos> = Vec::with_capacity(pts.len());
    out.push(pts[0]);
    for i in 1..pts.len() - 1 {
        let prev = *out.last().unwrap();
        let cur = pts[i];
        let next = pts[i + 1];
        let ax = cur.x - prev.x;
        let ay = cur.y - prev.y;
        let bx = next.x - cur.x;
        let by = next.y - cur.y;
        // Cross product magnitude = 2 * triangle area; small ⇒ collinear.
        let cross = (ax * by - ay * bx).abs();
        if cross > EPS {
            out.push(cur);
        }
    }
    out.push(pts[pts.len() - 1]);
    out
}

// ─── tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Modifiers;
    use crate::scene::{empty_node_data, Edge, EdgeId, Node, PortId, Port};

    fn mk_scene() -> Scene {
        let mut s = Scene::new();
        // A: (0,0)-(40,30), port "out" at right edge centre (40,15)
        s.insert_node(Node {
            id: NodeId(0),
            rect: Rect::from_min_size(Pos::new(0.0, 0.0), 40.0, 30.0),
            kind: "t".into(),
            data: empty_node_data(),
            ports: vec![Port {
                id: PortId::new("out"),
                local_offset: Pos::new(40.0, 15.0),
                kind: "".into(),
            }],
            label: "A".into(),
            origin: None,
            resizable: true,
            visual_rect: None,
        });
        // B: (100,0)-(140,30), port "in" at left edge centre (0,15)
        s.insert_node(Node {
            id: NodeId(1),
            rect: Rect::from_min_size(Pos::new(100.0, 0.0), 40.0, 30.0),
            kind: "t".into(),
            data: empty_node_data(),
            ports: vec![Port {
                id: PortId::new("in"),
                local_offset: Pos::new(0.0, 15.0),
                kind: "".into(),
            }],
            label: "B".into(),
            origin: None,
            resizable: true,
            visual_rect: None,
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
                SceneEvent::EdgeCreated { from, to, .. } => Some((from.clone(), to.clone())),
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
            data: empty_node_data(),
            origin: None,
            waypoints: Vec::new(),
            waypoints_authored: false,
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
