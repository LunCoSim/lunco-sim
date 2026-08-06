//! Pure projector: a composed USD stage → a `lunco_canvas::Scene`.
//!
//! Two stages, split so the interesting half is testable without a live
//! stage:
//!
//! - [`collect_graph`] reads the complete live `StageView` over the canonical
//!   stage into plain [`PrimNode`] / [`Wire`] structs. Thin glue over the same
//!   read API + connection-string split the co-sim wiring derivation uses
//!   (`lunco_usd_sim::cosim::rewire_usd_connections`).
//! - [`project_schema`] is an explicit presentation projection for the Lunica
//!   Schema perspective. It is driven by authored USD properties and never
//!   changes the collected topology used by simulation.
//! - [`build_scene`] is a **pure function** `(nodes, wires) → Scene`: it filters
//!   to the prims that actually participate in the graph, assigns a
//!   left-to-right dataflow layering, lays out ports, and emits nodes + edges.
//!   No USD, no Bevy — unit-tested directly.
//!
//! # What becomes a node vs an edge
//!
//! - **Node** — normally an active prim that has connectors (`inputs:*` /
//!   `outputs:*`) or is a rigid body (`PhysicsRigidBodyAPI`). A scene may
//!   author `lunco:ui:schemaNode = true` on system boundaries; the explicit
//!   [`project_schema`] function can then select those boundaries for the
//!   readable schema projection.
//! - **Dataflow edge** — one per authored `inputs:<c>.connect` (the co-sim wire:
//!   sink `inputs:` ← source `outputs:`). Drawn source-output → sink-input.
//! - **Joint edge** — one per prim carrying both `physics:body0` and
//!   `physics:body1`; the joint prim itself is rendered as the edge (not a node),
//!   connecting its two bodies.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use lunco_canvas::{empty_node_data, Edge, Node, Port, PortId, PortRef, Pos, Rect, Scene};
use lunco_usd_bevy::{SdfPath, StageView, UsdRead};

/// Node kind id registered in the canvas `VisualRegistry`.
pub(crate) const NODE_KIND: &str = "usd.prim";
/// Edge kind id registered in the canvas `VisualRegistry`.
pub(crate) const EDGE_KIND: &str = "usd.wire";

// Layout constants (world units). A node is a fixed card; ranks march right,
// rows march down. Wide enough to fit a prim leaf name + type label.
const NODE_W: f32 = 250.0;
const NODE_H: f32 = 96.0;
const PORT_ROW_H: f32 = 19.0;
const COL_SPACING: f32 = 360.0;
const ROW_SPACING: f32 = 230.0;
const ROW_GAP: f32 = 56.0;
const MARGIN: f32 = 40.0;

/// Whether a wire is a co-sim dataflow connection or a physics joint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WireKind {
    /// Authored `inputs:<c>.connect` — a co-sim signal wire.
    Dataflow,
    /// A joint prim's `physics:body0` ↔ `physics:body1`.
    Joint,
}

/// Typed payload carried in `Node.data` for `"usd.prim"` nodes; the visual
/// factory downcasts it.
#[derive(Clone, Debug)]
pub(crate) struct UsdPrimNodeData {
    pub type_name: String,
    /// Applies `PhysicsRigidBodyAPI` — drawn with the body accent.
    pub is_body: bool,
}

/// Typed payload carried in `Edge.data` for `"usd.wire"` edges.
#[derive(Clone, Debug)]
pub(crate) struct UsdWireData {
    pub kind: WireKind,
}

/// A prim read out of the stage, before layout.
#[derive(Clone, Debug)]
pub(crate) struct PrimNode {
    pub path: String,
    /// Standard USD `ui:displayName`, when authored; the path leaf is the
    /// deterministic fallback for assets that do not provide one.
    pub display_name: Option<String>,
    pub type_name: String,
    pub is_body: bool,
    /// Typed USD presentation markers copied from the composed stage. They
    /// are consumed only by [`project_schema`].
    pub schema_root: bool,
    pub schema_node: bool,
    /// Optional authored presentation column/row.  These are USD layout
    /// properties, not an engine-side classification of the prim.
    pub schema_column: Option<i32>,
    pub schema_row: Option<i32>,
    /// Connector leaf names (no `inputs:` prefix).
    pub inputs: Vec<String>,
    /// Connector leaf names (no `outputs:` prefix).
    pub outputs: Vec<String>,
}

/// A link read out of the stage, before resolution against the node set.
#[derive(Clone, Debug)]
pub(crate) struct Wire {
    pub kind: WireKind,
    pub source_path: String,
    /// Dataflow only — the producing connector leaf. Empty for joints.
    pub source_conn: String,
    pub target_path: String,
    /// Dataflow only — the consuming connector leaf. Empty for joints.
    pub target_conn: String,
}

/// Read every prim in `prim_paths` + its connections out of a composed stage.
///
/// `prim_paths` are the scene's prim path strings — supplied by the caller from
/// the ECS `UsdPrimPath` entities, exactly the enumeration
/// `rewire_usd_connections` uses (a live `StageView::prim_paths()` traversal can
/// miss composed children, so we key off the entities that were actually
/// spawned). `inputs:<c>` attrs are sinks, their `connections()` are the
/// producers, split at the last `.` into `(prim, connector-leaf)`. A prim
/// carrying both joint bodies becomes a joint wire and is NOT itself a node.
pub(crate) fn collect_graph(
    view: &StageView<'_>,
    prim_paths: &[String],
) -> (Vec<PrimNode>, Vec<Wire>) {
    let mut nodes: Vec<PrimNode> = Vec::new();
    let mut wires: Vec<Wire> = Vec::new();

    for path in prim_paths {
        let Ok(p) = SdfPath::new(path) else {
            continue;
        };
        if !view.is_active(&p) {
            continue;
        }
        let path = path.clone();

        // A prim with both bodies is a joint: render it as an edge between the
        // two bodies, not as a node.
        let body0 = view.rel_target(&p, "physics:body0");
        let body1 = view.rel_target(&p, "physics:body1");
        if let (Some(a), Some(b)) = (body0, body1) {
            wires.push(Wire {
                kind: WireKind::Joint,
                source_path: a,
                source_conn: String::new(),
                target_path: b,
                target_conn: String::new(),
            });
            continue;
        }

        let type_name = view.type_name(&p).unwrap_or_default();
        let display_name = view
            .text(&p, "ui:displayName")
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty());
        let is_body = view.has_api_schema(&p, "PhysicsRigidBodyAPI");
        let schema_root = view.scalar::<bool>(&p, "lunco:ui:schemaRoot") == Some(true);
        let schema_node = view.scalar::<bool>(&p, "lunco:ui:schemaNode") == Some(true);
        let schema_column = view.scalar::<i32>(&p, "lunco:ui:schemaColumn");
        let schema_row = view.scalar::<i32>(&p, "lunco:ui:schemaRow");
        let mut inputs: Vec<String> = Vec::new();
        let mut outputs: Vec<String> = Vec::new();

        for attr in view.attr_names(&p) {
            if let Some(conn) = attr.strip_prefix("inputs:") {
                inputs.push(conn.to_string());
                for src in view.connections(&p, &attr) {
                    // `/A.outputs:netForce` → prim `/A`, connector `netForce`.
                    let Some((src_prim, leaf)) = src.rsplit_once('.') else {
                        continue;
                    };
                    let src_conn = leaf
                        .strip_prefix("outputs:")
                        .or_else(|| leaf.strip_prefix("inputs:"))
                        .unwrap_or(leaf)
                        .to_string();
                    wires.push(Wire {
                        kind: WireKind::Dataflow,
                        source_path: src_prim.to_string(),
                        source_conn: src_conn,
                        target_path: path.clone(),
                        target_conn: conn.to_string(),
                    });
                }
            } else if let Some(conn) = attr.strip_prefix("outputs:") {
                outputs.push(conn.to_string());
            }
        }

        nodes.push(PrimNode {
            path,
            display_name,
            type_name,
            is_body,
            schema_root,
            schema_node,
            schema_column,
            schema_row,
            inputs,
            outputs,
        });
    }

    (nodes, wires)
}

/// Select the authored system boundaries for the Lunica Schema perspective.
///
/// This is intentionally separate from [`collect_graph`]. The canonical USD
/// graph must remain complete for simulation, editing, diagnostics, and any
/// future full-topology view. A schema projection is only a readable boundary
/// view: `lunco:ui:schemaRoot` scopes one instance and
/// `lunco:ui:schemaNode` marks the blocks that should be shown. Both are typed
/// USD properties; no path/name classification is performed here.
pub(crate) fn project_schema(
    mut nodes: Vec<PrimNode>,
    mut wires: Vec<Wire>,
) -> (Vec<PrimNode>, Vec<Wire>) {
    let schema_roots: BTreeSet<String> = nodes
        .iter()
        .filter(|node| node.schema_root)
        .map(|node| node.path.clone())
        .collect();
    let marked: BTreeSet<String> = nodes
        .iter()
        .filter(|node| {
            node.schema_node
                && (schema_roots.is_empty()
                    || schema_roots.iter().any(|root| {
                        node.path == *root
                            || node
                                .path
                                .strip_prefix(root)
                                .is_some_and(|rest| rest.starts_with('/'))
                    }))
        })
        .map(|node| node.path.clone())
        .collect();

    // If a scene has not authored a schema, preserve the generic complete
    // graph. This keeps the collector useful for ordinary USD scenes and makes
    // the schema perspective opt-in rather than an implicit global filter.
    if marked.is_empty() {
        return (nodes, wires);
    }

    nodes.retain(|node| marked.contains(&node.path));
    wires.retain(|wire| {
        // A same-prim forwarding binding is valid runtime topology, but not a
        // connection between two blocks. Hide it only in this presentation.
        wire.source_path != wire.target_path
            && marked.contains(&wire.source_path)
            && marked.contains(&wire.target_path)
    });

    for node in &mut nodes {
        node.inputs.retain(|name| {
            wires.iter().any(|wire| {
                wire.kind == WireKind::Dataflow
                    && wire.target_path == node.path
                    && wire.target_conn == *name
            })
        });
        node.outputs.retain(|name| {
            wires.iter().any(|wire| {
                wire.kind == WireKind::Dataflow
                    && wire.source_path == node.path
                    && wire.source_conn == *name
            })
        });
    }

    (nodes, wires)
}

/// Turn read prims + wires into a laid-out canvas [`Scene`]. Pure.
///
/// Keeps only prims that participate in the graph (have connectors or are
/// bodies), lays them out left-to-right by dataflow rank, and emits one canvas
/// node per prim (ports from the union of its own connectors and any connector a
/// wire names on it) and one edge per resolvable wire.
pub(crate) fn build_scene(nodes: Vec<PrimNode>, wires: Vec<Wire>) -> Scene {
    // Relevant = wiring-visible prims. Traversal order is preserved (stable,
    // deterministic layout across rebuilds).
    let relevant: Vec<PrimNode> = nodes
        .into_iter()
        .filter(|n| !n.inputs.is_empty() || !n.outputs.is_empty() || n.is_body)
        .collect();
    let n = relevant.len();

    let index: HashMap<String, usize> = relevant
        .iter()
        .enumerate()
        .map(|(i, node)| (node.path.clone(), i))
        .collect();

    // Drop wires whose endpoints aren't both nodes (e.g. a joint body that got
    // filtered, or a source prim not yet spawned).
    let wires: Vec<Wire> = wires
        .into_iter()
        .filter(|w| index.contains_key(&w.source_path) && index.contains_key(&w.target_path))
        .collect();

    // Port sets: seed from each prim's own connectors, then union in every
    // connector a dataflow wire references (so both endpoints of every edge have
    // a port to attach to even if the stage read missed the attr).
    let mut in_ports: Vec<BTreeSet<String>> = vec![BTreeSet::new(); n];
    let mut out_ports: Vec<BTreeSet<String>> = vec![BTreeSet::new(); n];
    for (i, node) in relevant.iter().enumerate() {
        in_ports[i].extend(node.inputs.iter().cloned());
        out_ports[i].extend(node.outputs.iter().cloned());
    }
    for w in &wires {
        if w.kind == WireKind::Dataflow {
            out_ports[index[&w.source_path]].insert(w.source_conn.clone());
            in_ports[index[&w.target_path]].insert(w.target_conn.clone());
        }
    }

    // Collapse real dataflow cycles into strongly connected components, then
    // rank the resulting DAG.  The old fixed-point relaxation promoted every
    // member of a feedback loop until the clamp, which made a cyclic Modelica
    // schema appear as one giant column.  SCC condensation is a graph-layout
    // operation, not a classification heuristic: authored USD connections are
    // still the sole source of topology.
    let rank = dataflow_ranks(n, &wires, &index);

    let node_heights: Vec<f32> = (0..n)
        .map(|i| {
            let port_count = in_ports[i].len().max(out_ports[i].len()) as f32;
            (NODE_H).max(46.0 + port_count * PORT_ROW_H)
        })
        .collect();

    // Position: authored schema columns/rows win. Unauthored nodes use the
    // deterministic dataflow rank and stable traversal order as a useful
    // fallback for generic scenes. Row spacing is expanded by the tallest card
    // in the previous authored row; a large port contract must never cover the
    // card below it.
    let mut rows_per_column: HashMap<i32, u32> = HashMap::new();
    let mut columns_rows: Vec<(i32, i32)> = Vec::with_capacity(n);
    let mut row_heights: HashMap<(i32, i32), f32> = HashMap::new();
    for i in 0..n {
        let column = relevant[i].schema_column.unwrap_or(rank[i]).max(0);
        let row = relevant[i].schema_row.unwrap_or_else(|| {
            let row = *rows_per_column.get(&column).unwrap_or(&0);
            rows_per_column.insert(column, row + 1);
            row as i32
        });
        let row = row.max(0);
        columns_rows.push((column, row));
        row_heights
            .entry((column, row))
            .and_modify(|height| *height = height.max(node_heights[i]))
            .or_insert(node_heights[i]);
    }
    let mut positions: Vec<Pos> = vec![Pos::default(); n];
    for i in 0..n {
        let (column, row) = columns_rows[i];
        let mut y = MARGIN;
        for previous_row in 0..row {
            y += row_heights
                .get(&(column, previous_row))
                .copied()
                .unwrap_or(ROW_SPACING)
                + ROW_GAP;
        }
        positions[i] = Pos::new(MARGIN + column as f32 * COL_SPACING, y);
    }

    let mut scene = Scene::new();
    let mut node_ids = Vec::with_capacity(n);
    for i in 0..n {
        let node = &relevant[i];
        let rect = Rect::from_min_size(positions[i], NODE_W, node_heights[i]);
        let mut ports: Vec<Port> = Vec::new();

        let ins: Vec<&String> = in_ports[i].iter().collect();
        for (k, name) in ins.iter().enumerate() {
            ports.push(Port {
                id: PortId::new((*name).clone()),
                local_offset: Pos::new(0.0, port_y(k, ins.len(), node_heights[i])),
                kind: "input".into(),
            });
        }
        let outs: Vec<&String> = out_ports[i].iter().collect();
        for (k, name) in outs.iter().enumerate() {
            ports.push(Port {
                id: PortId::new((*name).clone()),
                local_offset: Pos::new(NODE_W, port_y(k, outs.len(), node_heights[i])),
                kind: "output".into(),
            });
        }
        // Hidden joint anchors — `~jr` (right) sources a joint edge, `~jl` (left)
        // sinks it. Prefixed `~` so the visual skips painting them. Present on
        // every node so any joint edge resolves.
        ports.push(Port {
            id: PortId::new("~jr"),
            local_offset: Pos::new(NODE_W, node_heights[i] * 0.5),
            kind: "joint".into(),
        });
        ports.push(Port {
            id: PortId::new("~jl"),
            local_offset: Pos::new(0.0, node_heights[i] * 0.5),
            kind: "joint".into(),
        });

        let leaf = node
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&node.path)
            .to_string();
        let id = scene.alloc_node_id();
        scene.insert_node(Node {
            id,
            rect,
            kind: NODE_KIND.into(),
            data: Arc::new(UsdPrimNodeData {
                type_name: node.type_name.clone(),
                is_body: node.is_body,
            }),
            ports,
            label: node.display_name.clone().unwrap_or(leaf),
            origin: Some(node.path.clone()),
            resizable: false,
            visual_rect: None,
        });
        node_ids.push(id);
    }

    for w in &wires {
        let (s, t) = (index[&w.source_path], index[&w.target_path]);
        let from_world = match w.kind {
            WireKind::Dataflow => {
                output_port_world(s, &w.source_conn, &positions, &node_heights, &out_ports)
            }
            WireKind::Joint => Pos::new(
                positions[s].x + NODE_W,
                positions[s].y + node_heights[s] * 0.5,
            ),
        };
        let to_world = match w.kind {
            WireKind::Dataflow => {
                input_port_world(t, &w.target_conn, &positions, &node_heights, &in_ports)
            }
            WireKind::Joint => Pos::new(positions[t].x, positions[t].y + node_heights[t] * 0.5),
        };
        let (from, to) = match w.kind {
            WireKind::Dataflow => (
                PortRef {
                    node: node_ids[s],
                    port: PortId::new(w.source_conn.clone()),
                },
                PortRef {
                    node: node_ids[t],
                    port: PortId::new(w.target_conn.clone()),
                },
            ),
            WireKind::Joint => (
                PortRef {
                    node: node_ids[s],
                    port: PortId::new("~jr"),
                },
                PortRef {
                    node: node_ids[t],
                    port: PortId::new("~jl"),
                },
            ),
        };
        let eid = scene.alloc_edge_id();
        scene.insert_edge(Edge {
            id: eid,
            from,
            to,
            kind: EDGE_KIND.into(),
            data: Arc::new(UsdWireData { kind: w.kind }),
            origin: None,
            waypoints: if w.kind == WireKind::Dataflow {
                orthogonal_waypoints(from_world, to_world)
            } else {
                Vec::new()
            },
            waypoints_authored: false,
        });
    }

    let _ = empty_node_data; // (kept in scope for symmetry with scene.rs helpers)
    scene
}

fn output_port_world(
    node: usize,
    name: &str,
    positions: &[Pos],
    node_heights: &[f32],
    ports: &[BTreeSet<String>],
) -> Pos {
    let index = ports[node]
        .iter()
        .position(|port| port == name)
        .unwrap_or(0);
    Pos::new(
        positions[node].x + NODE_W,
        positions[node].y + port_y(index, ports[node].len(), node_heights[node]),
    )
}

fn input_port_world(
    node: usize,
    name: &str,
    positions: &[Pos],
    node_heights: &[f32],
    ports: &[BTreeSet<String>],
) -> Pos {
    let index = ports[node]
        .iter()
        .position(|port| port == name)
        .unwrap_or(0);
    Pos::new(
        positions[node].x,
        positions[node].y + port_y(index, ports[node].len(), node_heights[node]),
    )
}

/// Route a signal with two orthogonal segments. The midpoint is deterministic
/// from the endpoints, so the graph stays stable across rebuilds and can still
/// be edited by the canvas later. Reverse-direction edges use an outside lane
/// to keep feedback from being mistaken for forward dataflow.
fn orthogonal_waypoints(from: Pos, to: Pos) -> Vec<Pos> {
    if to.x <= from.x {
        // Feedback travels above the cards, where it cannot be mistaken for a
        // forward command or cross the port labels inside the graph.
        let lane_y = from.y.min(to.y) - 48.0;
        return vec![Pos::new(from.x, lane_y), Pos::new(to.x, lane_y)];
    }
    let mid_x = from.x + (to.x - from.x) * 0.5;
    vec![Pos::new(mid_x, from.y), Pos::new(mid_x, to.y)]
}

/// Longest-path ranks of the dataflow graph after collapsing strongly
/// connected components.  A feedback loop is one logical component, while
/// components downstream of it still receive a meaningful left-to-right rank.
fn dataflow_ranks(node_count: usize, wires: &[Wire], index: &HashMap<String, usize>) -> Vec<i32> {
    if node_count == 0 {
        return Vec::new();
    }

    let mut graph = vec![Vec::<usize>::new(); node_count];
    let mut reverse = vec![Vec::<usize>::new(); node_count];
    for wire in wires {
        if wire.kind != WireKind::Dataflow {
            continue;
        }
        let (Some(&source), Some(&target)) =
            (index.get(&wire.source_path), index.get(&wire.target_path))
        else {
            continue;
        };
        if !graph[source].contains(&target) {
            graph[source].push(target);
            reverse[target].push(source);
        }
    }

    fn visit(node: usize, graph: &[Vec<usize>], seen: &mut [bool], order: &mut Vec<usize>) {
        if seen[node] {
            return;
        }
        seen[node] = true;
        for &next in &graph[node] {
            visit(next, graph, seen, order);
        }
        order.push(node);
    }

    fn assign(node: usize, component: usize, reverse: &[Vec<usize>], components: &mut [usize]) {
        if components[node] != usize::MAX {
            return;
        }
        components[node] = component;
        for &next in &reverse[node] {
            assign(next, component, reverse, components);
        }
    }

    let mut seen = vec![false; node_count];
    let mut order = Vec::with_capacity(node_count);
    for node in 0..node_count {
        visit(node, &graph, &mut seen, &mut order);
    }

    let mut components = vec![usize::MAX; node_count];
    let mut component_count = 0;
    for &node in order.iter().rev() {
        if components[node] == usize::MAX {
            assign(node, component_count, &reverse, &mut components);
            component_count += 1;
        }
    }

    let mut condensation = BTreeSet::<(usize, usize)>::new();
    let mut indegree = vec![0usize; component_count];
    for source in 0..node_count {
        for &target in &graph[source] {
            let from = components[source];
            let to = components[target];
            if from != to && condensation.insert((from, to)) {
                indegree[to] += 1;
            }
        }
    }

    let mut ready = BTreeSet::new();
    for (component, &degree) in indegree.iter().enumerate() {
        if degree == 0 {
            ready.insert(component);
        }
    }
    let mut component_rank = vec![0i32; component_count];
    while let Some(component) = ready.pop_first() {
        for &(from, to) in condensation.range((component, 0)..=(component, usize::MAX)) {
            debug_assert_eq!(from, component);
            component_rank[to] = component_rank[to].max(component_rank[from] + 1);
            indegree[to] -= 1;
            if indegree[to] == 0 {
                ready.insert(to);
            }
        }
    }

    components
        .into_iter()
        .map(|component| component_rank[component])
        .collect()
}

/// Even vertical distribution of `count` ports down a node's dynamic edge:
/// port `k` sits at `H·(k+1)/(count+1)`.
fn port_y(k: usize, count: usize, height: f32) -> f32 {
    40.0 + PORT_ROW_H
        * (k as f32 + 0.5)
            .min((count.max(1) as f32) - 0.5)
            .min(height - 46.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prim(path: &str, ins: &[&str], outs: &[&str], is_body: bool) -> PrimNode {
        PrimNode {
            path: path.to_string(),
            display_name: None,
            type_name: "Xform".to_string(),
            is_body,
            schema_root: false,
            schema_node: false,
            schema_column: None,
            schema_row: None,
            inputs: ins.iter().map(|s| s.to_string()).collect(),
            outputs: outs.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn dataflow(src: &str, sc: &str, tgt: &str, tc: &str) -> Wire {
        Wire {
            kind: WireKind::Dataflow,
            source_path: src.to_string(),
            source_conn: sc.to_string(),
            target_path: tgt.to_string(),
            target_conn: tc.to_string(),
        }
    }

    /// A prim with neither connectors nor a body is not part of the wiring and
    /// must be dropped (an xform, a light, the terrain).
    #[test]
    fn irrelevant_prims_are_dropped() {
        let nodes = vec![
            prim("/Osc", &[], &["signal"], false),
            prim("/Terrain", &[], &[], false),
        ];
        let scene = build_scene(nodes, vec![]);
        assert_eq!(scene.node_count(), 1);
        let leaves: Vec<_> = scene.nodes().map(|(_, n)| n.label.clone()).collect();
        assert_eq!(leaves, vec!["Osc".to_string()]);
    }

    /// A body prim with no connectors is kept (it can still be a joint endpoint).
    #[test]
    fn body_without_connectors_is_kept() {
        let scene = build_scene(vec![prim("/Chassis", &[], &[], true)], vec![]);
        assert_eq!(scene.node_count(), 1);
    }

    /// The dataflow edge resolves to a real output port on the source and input
    /// port on the sink — the endpoints the write-back path reads back.
    #[test]
    fn dataflow_edge_resolves_to_named_ports() {
        let nodes = vec![
            prim("/Osc", &[], &["signal"], false),
            prim("/Amp", &["signal"], &["scaled"], false),
        ];
        let wires = vec![dataflow("/Osc", "signal", "/Amp", "signal")];
        let scene = build_scene(nodes, wires);
        assert_eq!(scene.edge_count(), 1);
        // Every edge's endpoints resolve to existing ports.
        for (_, e) in scene.edges() {
            assert!(
                scene.edge_endpoint_positions(e).is_some(),
                "edge endpoints must resolve to ports"
            );
        }
    }

    /// A connector referenced only by a wire (the stage read missed the sink's
    /// `inputs:` attr) still gets a port, so the edge never dangles. `/Amp`
    /// declares no inputs but survives the relevance filter via its output.
    #[test]
    fn wire_only_connector_still_gets_a_port() {
        let nodes = vec![
            prim("/Osc", &[], &["signal"], false),
            prim("/Amp", &[], &["scaled"], false),
        ];
        let wires = vec![dataflow("/Osc", "signal", "/Amp", "signal")];
        let scene = build_scene(nodes, wires);
        let amp = scene
            .nodes()
            .find(|(_, n)| n.label == "Amp")
            .map(|(_, n)| n)
            .expect("Amp node");
        assert!(
            amp.ports
                .iter()
                .any(|p| p.id.as_str() == "signal" && p.kind.as_str() == "input"),
            "sink must expose the wired input port"
        );
    }

    /// Layering: a pure source sits left of its sink (strictly smaller x).
    #[test]
    fn dataflow_layers_left_to_right() {
        let nodes = vec![
            prim("/Amp", &["signal"], &["scaled"], false),
            prim("/Osc", &[], &["signal"], false),
            prim("/Sink", &["scaled"], &[], false),
        ];
        let wires = vec![
            dataflow("/Osc", "signal", "/Amp", "signal"),
            dataflow("/Amp", "scaled", "/Sink", "scaled"),
        ];
        let scene = build_scene(nodes, wires);
        let x = |leaf: &str| {
            scene
                .nodes()
                .find(|(_, n)| n.label == leaf)
                .map(|(_, n)| n.rect.min.x)
                .unwrap()
        };
        assert!(x("/Osc".trim_start_matches('/')) < x("Amp"));
        assert!(x("Amp") < x("Sink"));
    }

    /// A joint prim (both bodies) becomes an edge between them; the two bodies
    /// are the only nodes.
    #[test]
    fn joint_becomes_edge_between_bodies() {
        let nodes = vec![prim("/A", &[], &[], true), prim("/B", &[], &[], true)];
        let wires = vec![Wire {
            kind: WireKind::Joint,
            source_path: "/A".to_string(),
            source_conn: String::new(),
            target_path: "/B".to_string(),
            target_conn: String::new(),
        }];
        let scene = build_scene(nodes, wires);
        assert_eq!(scene.node_count(), 2);
        assert_eq!(scene.edge_count(), 1);
        for (_, e) in scene.edges() {
            assert!(scene.edge_endpoint_positions(e).is_some());
        }
    }

    /// A cycle doesn't hang the layering and every node still gets a bounded rank.
    #[test]
    fn cyclic_dataflow_terminates_and_bounds_rank() {
        let nodes = vec![
            prim("/A", &["x"], &["y"], false),
            prim("/B", &["y"], &["x"], false),
        ];
        let wires = vec![
            dataflow("/A", "y", "/B", "y"),
            dataflow("/B", "x", "/A", "x"),
        ];
        let scene = build_scene(nodes, wires);
        assert_eq!(scene.node_count(), 2);
        // Ranks are clamped to < n, so x stays within one column span of margin.
        for (_, node) in scene.nodes() {
            assert!(node.rect.min.x <= MARGIN + (2.0 - 1.0) * COL_SPACING + 0.5);
        }
    }

    /// The USD reader stays complete; only the explicit schema projection
    /// selects authored boundaries and removes same-prim forwarding bindings.
    #[test]
    fn schema_projection_is_explicit_and_property_driven() {
        let mut root = prim("/Lander", &[], &["out"], false);
        root.schema_root = true;
        root.schema_node = true;
        let mut controller = prim("/Lander/GNC", &["in"], &["cmd"], false);
        controller.schema_node = true;
        let internal = prim("/Lander/Internal", &["state"], &["state"], false);
        let wires = vec![
            dataflow("/Lander", "out", "/Lander/GNC", "in"),
            dataflow("/Lander", "state", "/Lander", "state"),
        ];

        let (nodes, wires) = project_schema(vec![root, controller, internal], wires);
        assert_eq!(
            nodes
                .iter()
                .map(|node| node.path.as_str())
                .collect::<Vec<_>>(),
            vec!["/Lander", "/Lander/GNC"]
        );
        assert_eq!(wires.len(), 1);
        assert_eq!(wires[0].source_path, "/Lander");
        assert_eq!(wires[0].target_path, "/Lander/GNC");
    }
}
