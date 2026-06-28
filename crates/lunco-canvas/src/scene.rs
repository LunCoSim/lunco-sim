//! The scene graph — the canvas's authored state.
//!
//! Pure data, no rendering. A `Scene` is a flat collection of
//! [`Node`]s and [`Edge`]s expressed in **world coordinates**; a
//! [`Viewport`](crate::viewport::Viewport) transforms world → screen
//! at render time.
//!
//! # Separation of concerns
//!
//! `Scene` holds **what exists** (identities, positions, connection
//! topology). It does NOT hold:
//! - how things *look* (that's a per-node/per-edge `Box<dyn NodeVisual>` /
//!   `Box<dyn EdgeVisual>` — see [`crate::visual`])
//! - how things *interact* (that's `Tool` — see [`crate::tool`])
//! - how things *animate* (that's carried via `DrawCtx` + the tool's
//!   frame loop — visuals opt in)
//!
//! # Why identifiers are newtyped `u64`s
//!
//! A bare index into a `Vec<Node>` would invalidate on any delete; a
//! `String` key would cost allocation per lookup. `NodeId(u64)` is
//! copy-cheap, lookup-stable (we use `IndexMap` so iteration is
//! deterministic without any sort), and serializable.
//!
//! # Serializable on day 1
//!
//! All structs here derive `Serialize + Deserialize`. That's load-
//! bearing for two future features:
//!
//! - `SceneDocument` in `lunco-doc` — when composition / dataflow
//!   graphs / standalone annotations need a `.lcscene` file, they'll
//!   serialise `Scene` as-is.
//! - Copy/paste and undo/redo snapshots — the caller can snapshot a
//!   `Scene` into a document op without any canvas-specific plumbing.
//!
//! `Box<dyn NodeVisual>` can't be serialised directly; instead, every
//! node stores a **kind id** (`SmolStr`, e.g. `"modelica.icon"`) plus an
//! opaque, type-erased payload ([`NodeData`] = `Arc<dyn Any + Send +
//! Sync>`, `#[serde(skip)]`). At load time the crate's `VisualRegistry`
//! rebuilds the trait object from the kind, downcasting the payload to
//! its concrete struct. See [`crate::visual::VisualRegistry`].

use std::any::Any;
use std::collections::BTreeSet;
use std::sync::Arc;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

/// Type-erased per-node payload. Domain crates box their own typed
/// struct (e.g. `IconNodeData` in lunco-modelica) and visuals
/// downcast at draw time via `data.downcast_ref::<MyType>()`. `Arc`
/// makes Node/Edge cloneable without forcing payloads to implement
/// custom clone-boxing.
///
/// We deliberately don't carry serde derives here — Scene snapshot
/// for save/restore would need a per-domain serializer registry,
/// which is a future concern (no consumer of `Scene::serialize` is
/// active today). The `data` fields below are `#[serde(skip)]`.
pub type NodeData = Arc<dyn Any + Send + Sync>;

/// Construct an empty type-erased payload — used when a node/edge
/// carries no domain-specific data, and when a serde-deserialized
/// Scene needs a placeholder until the host re-attaches typed data.
pub fn empty_node_data() -> NodeData {
    Arc::new(())
}

/// Process-global structural-revision source for [`Scene::generation`].
/// Using one monotonic counter across *all* scenes (rather than a
/// per-scene `+= 1`) guarantees a value never repeats, so a render
/// layer that cached state from scene A can never get a false cache
/// hit when scene B is swapped in — B's revision is necessarily a
/// fresh, larger number (CQ-202). Starts at 1 so the `Default` scene's
/// `generation == 0` is distinguishable from any real revision.
static SCENE_REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_revision() -> u64 {
    SCENE_REVISION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Hit-test kind returned by [`Scene::hit_node`]. Mirrors
/// [`crate::visual::NodeHit`] but is defined here so the scene
/// module doesn't pull in the visual module just for one enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeHitKind {
    Body,
    Port(PortId),
}

/// Hit-test kind for edges — which part of the wire polyline the
/// pointer is over. Same rationale as [`NodeHitKind`]: defined in the
/// scene module so geometry-only callers don't have to import
/// `visual`. The [`crate::visual::EdgeHit`] alias re-exports this for
/// visual-side users.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeHitKind {
    /// On the wire but not on a specific handle.
    Body,
    /// On the i-th *interior* waypoint of the polyline.
    Corner(usize),
    /// On the midpoint of the i-th segment of the polyline. Segment 0
    /// connects the source port to the first interior waypoint (or
    /// directly to the target port when none exist).
    Segment(usize),
}

/// Squared perpendicular distance from `p` to the finite segment
/// `(a,b)`. Endpoint-clamped. Mirror of the one in `visual.rs`; kept
/// private to scene so the two modules stay independent.
fn perpendicular_dist_sq(p: Pos, a: Pos, b: Pos) -> f32 {
    let ax = b.x - a.x;
    let ay = b.y - a.y;
    let len_sq = ax * ax + ay * ay;
    if len_sq < f32::EPSILON {
        let dx = p.x - a.x;
        let dy = p.y - a.y;
        return dx * dx + dy * dy;
    }
    let t = (((p.x - a.x) * ax + (p.y - a.y) * ay) / len_sq).clamp(0.0, 1.0);
    let foot_x = a.x + t * ax;
    let foot_y = a.y + t * ay;
    let dx = p.x - foot_x;
    let dy = p.y - foot_y;
    dx * dx + dy * dy
}

/// Stable identifier for a [`Node`] within a single [`Scene`].
///
/// Allocated monotonically by the owning `Scene`; never reused even
/// after a delete, so a dangling reference from an old selection
/// reliably resolves to `None` instead of silently pointing at a new
/// node that happens to have taken the slot.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct NodeId(pub u64);

/// Stable identifier for an [`Edge`] within a single [`Scene`].
///
/// Same allocation discipline as [`NodeId`] — monotonic, not reused.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct EdgeId(pub u64);

/// Identifier for a [`Port`] **within a single node**.
///
/// Unlike `NodeId`/`EdgeId`, this is scoped to its parent node — two
/// different nodes both have `PortId(0)`. Strings are used (not `u32`)
/// because ports carry domain meaning (`"heatPort"`, `"pin_p"`) that
/// survives serialisation and shows up in user-visible error messages.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct PortId(pub SmolStr);

impl PortId {
    pub fn new(s: impl Into<SmolStr>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// A 2D point in **world coordinates**.
///
/// We don't reuse `egui::Pos2` here so the crate can be used outside
/// an egui context (tests, CI tooling, headless layout passes). The
/// conversion in [`crate::viewport`] is a zero-cost field rename.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Pos {
    pub x: f32,
    pub y: f32,
}

impl Pos {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// Axis-aligned rectangle in world coordinates: `[min, max]`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub min: Pos,
    pub max: Pos,
}

impl Rect {
    pub const fn from_min_max(min: Pos, max: Pos) -> Self {
        Self { min, max }
    }

    /// Construct from top-left + size.
    pub fn from_min_size(min: Pos, size_x: f32, size_y: f32) -> Self {
        Self {
            min,
            max: Pos::new(min.x + size_x, min.y + size_y),
        }
    }

    pub fn width(&self) -> f32 {
        self.max.x - self.min.x
    }
    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }
    pub fn center(&self) -> Pos {
        Pos::new(
            (self.min.x + self.max.x) * 0.5,
            (self.min.y + self.max.y) * 0.5,
        )
    }

    /// Does `p` lie in the closed rectangle? Used by default hit-testing.
    pub fn contains(&self, p: Pos) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }

    /// Translate `self` by `dx, dy`. Used by node-drag and scene-wide
    /// nudge operations.
    pub fn translated(self, dx: f32, dy: f32) -> Self {
        Self {
            min: Pos::new(self.min.x + dx, self.min.y + dy),
            max: Pos::new(self.max.x + dx, self.max.y + dy),
        }
    }

    /// Smallest rectangle that contains both `self` and `other`.
    /// Returns `other` if `self` is empty/degenerate; useful for
    /// `fit_all` where we fold an iterator.
    pub fn union(self, other: Rect) -> Rect {
        Rect {
            min: Pos::new(self.min.x.min(other.min.x), self.min.y.min(other.min.y)),
            max: Pos::new(self.max.x.max(other.max.x), self.max.y.max(other.max.y)),
        }
    }
}

/// A declared connection point on a [`Node`].
///
/// Ports are data, not visuals. The node's [`crate::visual::NodeVisual`]
/// decides where to paint them and how to hit-test them; the scene
/// only records identity, local offset (for routing), and an optional
/// kind string that callers use for validation (e.g. Modelica
/// connector-compatibility rules).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Port {
    pub id: PortId,
    /// Port attachment point **relative to the node's top-left**, in
    /// world units. The edge router reads this so wires meet the port
    /// graphic, not the bounding box corner.
    pub local_offset: Pos,
    /// Optional domain tag: `"electrical.pin"`, `"modelica.flange"`,
    /// `"dataflow.f32"`. Free-form, caller validates. Empty string
    /// means "untyped".
    pub kind: SmolStr,
}

/// A node in the scene.
///
/// Identity, world-space rect, port layout, and a kind + opaque data
/// blob that the visual registry uses to rebuild the visual on load.
/// Selection state is *not* stored here — it lives in
/// [`crate::selection::Selection`] so snapshot/restore of scene data
/// doesn't clobber the user's current highlight.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub rect: Rect,
    /// Kind identifier, e.g. `"modelica.icon"`. Looked up in the
    /// [`crate::visual::VisualRegistry`] to reconstruct the visual.
    pub kind: SmolStr,
    /// Type-erased per-kind payload. The visual's factory downcasts
    /// to its concrete payload struct (e.g.
    /// `IconNodeData` in lunco-modelica). `#[serde(skip)]` because
    /// `dyn Any` can't round-trip through serde — Scene save/restore
    /// is a future feature that will need a per-domain
    /// serializer registry.
    #[serde(skip, default = "empty_node_data")]
    pub data: NodeData,
    pub ports: Vec<Port>,
    /// User-editable display name. Defaults empty; the visual may
    /// choose to render it or ignore it.
    #[serde(default)]
    pub label: String,
    /// Optional back-reference into an upstream domain store
    /// (e.g. `"battery.cell1"`). The canvas never interprets it; the
    /// caller uses it to route scene events back to the right doc
    /// without maintaining a side table. Opaque string keeps the
    /// crate free of any domain dependency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// Whether the bottom-right resize handle is active for this node.
    /// Plot / control / dashboard nodes are resizable; Modelica
    /// component icons keep the size declared in their `Icon`
    /// annotation (resizing them would desync from the source). Default
    /// `true` so legacy code paths that build `Node` directly aren't
    /// silently turned non-resizable.
    #[serde(default = "default_resizable")]
    pub resizable: bool,
    /// Tight bounding box of the *visible* graphics, in world coords.
    /// Layers that draw a halo around the node — selection outline,
    /// hover, focus ring — should prefer this over `rect` so the halo
    /// fits the actual icon and not the placement extent (Modelica
    /// icons typically only fill ~50 % of their placement). `None`
    /// means "no tight bounds reported, fall back to `rect`".
    #[serde(default)]
    pub visual_rect: Option<Rect>,
}

fn default_resizable() -> bool {
    true
}

/// Reference to a specific port on a specific node. Edge endpoints
/// use this so a connection survives the port list reordering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortRef {
    pub node: NodeId,
    pub port: PortId,
}

/// An edge / connection between two ports.
///
/// Same `kind` + `data` pattern as [`Node`] so edge visuals
/// (orthogonal, bezier, animated) all slot through the same registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub id: EdgeId,
    pub from: PortRef,
    pub to: PortRef,
    pub kind: SmolStr,
    /// Type-erased payload — see [`Node::data`].
    #[serde(skip, default = "empty_node_data")]
    pub data: NodeData,
    /// Back-reference mirroring [`Node::origin`] — opaque string the
    /// caller uses to key against its own store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// Interior waypoints of the wire's polyline, in world coordinates.
    /// `Vec::new()` = no authored bends (the visual decides whether to
    /// auto-route). Owned by the scene so [`crate::tool::Tool`] can
    /// mutate them during a drag without reaching into per-domain edge
    /// `data` payloads. Endpoints (port positions) are NOT included.
    #[serde(default)]
    pub waypoints: Vec<Pos>,
    /// True when [`Self::waypoints`] came from a source-level
    /// annotation (the user / domain authored them) rather than from
    /// an auto-router. Hosts use this to gate persistence: rubber-band
    /// of incident wires on a node move should only emit an authored-
    /// path op when the wire was authored to begin with, otherwise
    /// every node drag would freeze the auto-router's output into the
    /// source annotation.
    #[serde(default)]
    pub waypoints_authored: bool,
}

/// The canvas's authored state.
///
/// `Scene` owns the nodes and edges; id allocation is monotonic and
/// scoped to this scene. Iteration order is insertion order (via
/// [`IndexMap`]) so saving and reloading produces a byte-identical
/// serialisation — important for the `SceneDocument` undo/redo story
/// where we diff serialised snapshots.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Scene {
    nodes: IndexMap<NodeId, Node>,
    edges: IndexMap<EdgeId, Edge>,
    /// Monotonic id counter — persisted so reload + add doesn't
    /// accidentally reuse a deleted id. The counter walks forward
    /// through both node and edge id allocations; using one shared
    /// counter means we never have to add a separate "next edge id"
    /// field on format bumps.
    #[serde(default)]
    next_id: u64,
    /// Structural revision (CQ-202). Bumped from [`next_revision`] by
    /// every mutator; render layers cache topology-derived state (the
    /// edge endpoint-incidence map) keyed on it and recompute only on a
    /// change. `#[serde(skip)]`: it's a runtime cache key, not authored
    /// state, and must not perturb the byte-identical snapshot diff used
    /// for undo — a deserialized scene resets to 0 and the next mutation
    /// re-stamps it (and the layer's sentinel forces a first-draw build).
    #[serde(skip)]
    generation: u64,
}

impl Scene {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn alloc_node_id(&mut self) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        id
    }
    pub fn alloc_edge_id(&mut self) -> EdgeId {
        let id = EdgeId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Current structural revision (CQ-202) — see [`Scene::generation`].
    /// Render layers compare this against their last-built value to skip
    /// recomputing topology-derived caches on unchanged frames.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn insert_node(&mut self, node: Node) -> NodeId {
        let id = node.id;
        self.nodes.insert(id, node);
        self.generation = next_revision();
        id
    }
    pub fn insert_edge(&mut self, edge: Edge) -> EdgeId {
        let id = edge.id;
        self.edges.insert(id, edge);
        self.generation = next_revision();
        id
    }

    /// Remove a node and all edges that referenced it. Returns the
    /// removed node (if any) plus the ids of edges that were cleaned
    /// up as fallout — callers emit a single `NodeDeleted` event plus
    /// `EdgeDeleted` events per orphan, preserving the undo diff.
    pub fn remove_node(&mut self, id: NodeId) -> Option<(Node, Vec<EdgeId>)> {
        let node = self.nodes.shift_remove(&id)?;
        let orphan_edges: Vec<EdgeId> = self
            .edges
            .iter()
            .filter_map(|(eid, e)| {
                if e.from.node == id || e.to.node == id {
                    Some(*eid)
                } else {
                    None
                }
            })
            .collect();
        for eid in &orphan_edges {
            self.edges.shift_remove(eid);
        }
        self.generation = next_revision();
        Some((node, orphan_edges))
    }

    pub fn remove_edge(&mut self, id: EdgeId) -> Option<Edge> {
        let removed = self.edges.shift_remove(&id);
        if removed.is_some() {
            self.generation = next_revision();
        }
        removed
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }
    /// Mutable node access. Conservatively bumps [`Scene::generation`] —
    /// the caller is assumed to mutate (rect/ports/data), so any cached
    /// topology-derived state must be considered stale (CQ-202).
    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        if self.nodes.contains_key(&id) {
            self.generation = next_revision();
            self.nodes.get_mut(&id)
        } else {
            None
        }
    }
    pub fn edge(&self, id: EdgeId) -> Option<&Edge> {
        self.edges.get(&id)
    }
    /// Mutable edge access. Conservatively bumps [`Scene::generation`]
    /// (the caller may reconnect endpoints, which changes incidence).
    pub fn edge_mut(&mut self, id: EdgeId) -> Option<&mut Edge> {
        if self.edges.contains_key(&id) {
            self.generation = next_revision();
            self.edges.get_mut(&id)
        } else {
            None
        }
    }

    pub fn nodes(&self) -> impl Iterator<Item = (&NodeId, &Node)> {
        self.nodes.iter()
    }
    pub fn edges(&self) -> impl Iterator<Item = (&EdgeId, &Edge)> {
        self.edges.iter()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Bounding rectangle of every node in the scene, or `None` if
    /// empty. Used by `fit_all`.
    pub fn bounds(&self) -> Option<Rect> {
        let mut iter = self.nodes.values().map(|n| n.rect);
        let first = iter.next()?;
        Some(iter.fold(first, Rect::union))
    }

    /// Data-driven hit test — returns which node (and which part of
    /// it) the world-space point lies over, walking in *reverse*
    /// insertion order so later-added nodes win when overlapping
    /// (the Figma / OS "top window" convention).
    ///
    /// Ports are tested as circles of `port_radius` world-units around
    /// their `local_offset`; bodies are `Rect::contains`. This matches
    /// the default [`crate::visual::NodeVisual::hit`] impl, so it's
    /// correct for any visual that hasn't overridden `hit`. Custom
    /// visuals with non-rectangular bodies will want to pre-filter
    /// here then refine via the visual trait — that plumbing lands
    /// when a real case needs it.
    pub fn hit_node(
        &self,
        world_pos: Pos,
        port_radius: f32,
    ) -> Option<(NodeId, NodeHitKind)> {
        let radius_sq = port_radius * port_radius;
        for (id, node) in self.nodes.iter().rev() {
            for port in &node.ports {
                let px = node.rect.min.x + port.local_offset.x;
                let py = node.rect.min.y + port.local_offset.y;
                let dx = world_pos.x - px;
                let dy = world_pos.y - py;
                if dx * dx + dy * dy <= radius_sq {
                    return Some((*id, NodeHitKind::Port(port.id.clone())));
                }
            }
            if node.rect.contains(world_pos) {
                return Some((*id, NodeHitKind::Body));
            }
        }
        None
    }

    /// Walk an edge's full polyline (port → interior waypoints → port)
    /// and report the closest segment's perpendicular squared distance
    /// to `world_pos`, plus the index of that segment. Returns `None`
    /// if the edge's endpoints can't be resolved. Used by both
    /// [`Self::hit_edge`] (threshold check) and the future
    /// waypoint-handle hit tests that need to know *which* segment was
    /// hit. Endpoints are at indices 0 and `1 + waypoints.len()`.
    pub fn edge_polyline_closest_segment(
        &self,
        edge: &Edge,
        world_pos: Pos,
    ) -> Option<(usize, f32)> {
        let (from, to) = self.edge_endpoint_positions(edge)?;
        let mut pts: Vec<Pos> = Vec::with_capacity(2 + edge.waypoints.len());
        pts.push(from);
        pts.extend(edge.waypoints.iter().copied());
        pts.push(to);
        let mut best: Option<(usize, f32)> = None;
        for i in 0..pts.len() - 1 {
            let d = perpendicular_dist_sq(world_pos, pts[i], pts[i + 1]);
            if best.as_ref().map_or(true, |(_, bd)| d < *bd) {
                best = Some((i, d));
            }
        }
        best
    }

    /// World-space port positions of an edge's two endpoints. Returns
    /// `None` if either node or port is missing — callers should treat
    /// that as "skip this edge".
    pub fn edge_endpoint_positions(&self, edge: &Edge) -> Option<(Pos, Pos)> {
        let from_node = self.nodes.get(&edge.from.node)?;
        let to_node = self.nodes.get(&edge.to.node)?;
        let from_port = from_node.ports.iter().find(|p| p.id == edge.from.port)?;
        let to_port = to_node.ports.iter().find(|p| p.id == edge.to.port)?;
        Some((
            Pos::new(
                from_node.rect.min.x + from_port.local_offset.x,
                from_node.rect.min.y + from_port.local_offset.y,
            ),
            Pos::new(
                to_node.rect.min.x + to_port.local_offset.x,
                to_node.rect.min.y + to_port.local_offset.y,
            ),
        ))
    }

    /// Data-driven edge hit test — returns the first edge within
    /// `threshold` world-units of `world_pos`, walking in reverse
    /// insertion order. Honours interior waypoints: a wire with bends
    /// only "clicks" when the cursor is near one of its actual
    /// segments, not when it sits diagonally between the two ports.
    pub fn hit_edge(&self, world_pos: Pos, threshold: f32) -> Option<EdgeId> {
        self.hit_edge_kind(world_pos, threshold, false, 0.0)
            .map(|(id, _)| id)
    }

    /// Like [`Self::hit_edge`] but classifies the hit into corner /
    /// segment / body. When `with_handles` is true and `handle_radius`
    /// is positive, interior-waypoint hits within `handle_radius` and
    /// segment-midpoint hits within `handle_radius` (along the segment
    /// axis perpendicular) take priority over plain body hits. Caller
    /// passes `with_handles = true` only when the matching edge is
    /// selected — handles are only drawn on selected edges, so they
    /// shouldn't grab clicks otherwise.
    pub fn hit_edge_kind(
        &self,
        world_pos: Pos,
        threshold: f32,
        with_handles: bool,
        handle_radius: f32,
    ) -> Option<(EdgeId, EdgeHitKind)> {
        let thr_sq = threshold * threshold;
        let handle_sq = handle_radius * handle_radius;
        for (id, edge) in self.edges.iter().rev() {
            let Some((from, to)) = self.edge_endpoint_positions(edge) else { continue };
            // Handle checks first (corner > segment midpoint > body).
            if with_handles && handle_radius > 0.0 {
                // Corner handles on interior waypoints.
                for (i, w) in edge.waypoints.iter().enumerate() {
                    let dx = world_pos.x - w.x;
                    let dy = world_pos.y - w.y;
                    if dx * dx + dy * dy <= handle_sq {
                        return Some((*id, EdgeHitKind::Corner(i)));
                    }
                }
                // Segment-midpoint handles.
                let mut pts: Vec<Pos> = Vec::with_capacity(2 + edge.waypoints.len());
                pts.push(from);
                pts.extend(edge.waypoints.iter().copied());
                pts.push(to);
                for i in 0..pts.len() - 1 {
                    let mx = (pts[i].x + pts[i + 1].x) * 0.5;
                    let my = (pts[i].y + pts[i + 1].y) * 0.5;
                    let dx = world_pos.x - mx;
                    let dy = world_pos.y - my;
                    if dx * dx + dy * dy <= handle_sq {
                        // Skip degenerate segments (<= 2 * handle_radius long).
                        let sx = pts[i + 1].x - pts[i].x;
                        let sy = pts[i + 1].y - pts[i].y;
                        if sx * sx + sy * sy > (4.0 * handle_sq) {
                            return Some((*id, EdgeHitKind::Segment(i)));
                        }
                    }
                }
            }
            // Fall back to plain body hit.
            if let Some((_seg, d)) = self.edge_polyline_closest_segment(edge, world_pos) {
                if d <= thr_sq {
                    return Some((*id, EdgeHitKind::Body));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_node(scene: &mut Scene, x: f32, y: f32) -> NodeId {
        let id = scene.alloc_node_id();
        scene.insert_node(Node {
            id,
            rect: Rect::from_min_size(Pos::new(x, y), 40.0, 30.0),
            kind: "test".into(),
            data: empty_node_data(),
            ports: vec![Port {
                id: PortId::new("out"),
                local_offset: Pos::new(40.0, 15.0),
                kind: "".into(),
            }],
            label: String::new(),
            origin: None,
            resizable: true,
            visual_rect: None,
        });
        id
    }

    fn mk_edge(scene: &mut Scene, from: NodeId, to: NodeId) -> EdgeId {
        let id = scene.alloc_edge_id();
        scene.insert_edge(Edge {
            id,
            from: PortRef {
                node: from,
                port: PortId::new("out"),
            },
            to: PortRef {
                node: to,
                port: PortId::new("out"),
            },
            kind: "test".into(),
            data: empty_node_data(),
            origin: None,
            waypoints: Vec::new(),
            waypoints_authored: false,
        });
        id
    }

    #[test]
    fn rect_contains_closed_interval() {
        let r = Rect::from_min_size(Pos::new(0.0, 0.0), 10.0, 10.0);
        assert!(r.contains(Pos::new(0.0, 0.0))); // min corner
        assert!(r.contains(Pos::new(10.0, 10.0))); // max corner
        assert!(r.contains(Pos::new(5.0, 5.0)));
        assert!(!r.contains(Pos::new(11.0, 5.0)));
        assert!(!r.contains(Pos::new(-0.1, 5.0)));
    }

    #[test]
    fn rect_union_of_disjoint_covers_both() {
        let a = Rect::from_min_size(Pos::new(0.0, 0.0), 10.0, 10.0);
        let b = Rect::from_min_size(Pos::new(20.0, 30.0), 5.0, 5.0);
        let u = a.union(b);
        assert_eq!(u.min, Pos::new(0.0, 0.0));
        assert_eq!(u.max, Pos::new(25.0, 35.0));
    }

    #[test]
    fn ids_are_monotonic_and_unique_across_kinds() {
        // Same counter backs both — a node and an edge never share an
        // allocation frame, but they also never collide within a
        // format bump. Check that.
        let mut s = Scene::new();
        let n0 = s.alloc_node_id();
        let e0 = s.alloc_edge_id();
        let n1 = s.alloc_node_id();
        assert_eq!(n0.0, 0);
        assert_eq!(e0.0, 1);
        assert_eq!(n1.0, 2);
    }

    #[test]
    fn removing_node_drops_touching_edges() {
        let mut s = Scene::new();
        let a = mk_node(&mut s, 0.0, 0.0);
        let b = mk_node(&mut s, 100.0, 0.0);
        let c = mk_node(&mut s, 200.0, 0.0);
        let ab = mk_edge(&mut s, a, b);
        let bc = mk_edge(&mut s, b, c);

        let (_removed, orphans) = s.remove_node(b).expect("b should exist");
        // Both edges referenced b; both must be gone.
        assert!(orphans.contains(&ab));
        assert!(orphans.contains(&bc));
        assert_eq!(orphans.len(), 2);
        assert_eq!(s.edge_count(), 0);
        assert_eq!(s.node_count(), 2);
    }

    #[test]
    fn bounds_covers_all_nodes() {
        let mut s = Scene::new();
        assert!(s.bounds().is_none());
        mk_node(&mut s, 0.0, 0.0); // 40×30
        mk_node(&mut s, 100.0, 50.0); // 40×30
        let b = s.bounds().expect("non-empty");
        assert_eq!(b.min, Pos::new(0.0, 0.0));
        assert_eq!(b.max, Pos::new(140.0, 80.0));
    }

    #[test]
    fn serde_roundtrip_preserves_order() {
        let mut s = Scene::new();
        let a = mk_node(&mut s, 0.0, 0.0);
        let b = mk_node(&mut s, 10.0, 0.0);
        mk_edge(&mut s, a, b);
        let json = serde_json::to_string(&s).unwrap();
        let back: Scene = serde_json::from_str(&json).unwrap();
        let ids: Vec<_> = back.nodes().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![a, b]); // insertion order preserved
    }

    #[test]
    fn iteration_deterministic_after_remove() {
        // IndexMap's shift_remove keeps the remaining order stable —
        // this is the contract save/load relies on for diff-clean
        // serialisation.
        let mut s = Scene::new();
        let a = mk_node(&mut s, 0.0, 0.0);
        let b = mk_node(&mut s, 10.0, 0.0);
        let c = mk_node(&mut s, 20.0, 0.0);
        s.remove_node(b);
        let ids: Vec<_> = s.nodes().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![a, c]);
    }
}
