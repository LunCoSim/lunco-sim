//! Pure projector: a composed USD stage → a `lunco_canvas::Scene`.
//!
//! Two stages, split so the interesting half is testable without a live
//! stage:
//!
//! - [`collect_graph`] reads the live `StageView` over the canonical stage
//!   into plain [`PrimNode`] / [`Wire`] structs. Thin
//!   glue over the same read API + connection-string split the co-sim wiring
//!   derivation uses (`lunco_usd_sim::cosim::rewire_usd_connections`).
//! - [`build_scene`] is a **pure function** `(nodes, wires) → Scene`: it filters
//!   to the prims that actually participate in the graph, assigns a
//!   left-to-right dataflow layering, lays out ports, and emits nodes + edges.
//!   No USD, no Bevy — unit-tested directly.
//!
//! # What becomes a node vs an edge
//!
//! - **Node** — an active prim that has connectors (`inputs:*` / `outputs:*`)
//!   or is a rigid body (`PhysicsRigidBodyAPI`). Xforms, cameras, lights, scopes
//!   are dropped so the canvas shows the wiring, not the whole scene tree.
//! - **Dataflow edge** — one per authored `inputs:<c>.connect` (the co-sim wire:
//!   sink `inputs:` ← source `outputs:`). Drawn source-output → sink-input.
//! - **Joint edge** — one per prim carrying both `physics:body0` and
//!   `physics:body1`; the joint prim itself is rendered as the edge (not a node),
//!   connecting its two bodies.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use lunco_canvas::{
    empty_node_data, Edge, Node, Pos, Port, PortId, PortRef, Rect, Scene,
};
use lunco_usd_bevy::{SdfPath, StageView, UsdRead};

/// Node kind id registered in the canvas `VisualRegistry`.
pub(crate) const NODE_KIND: &str = "usd.prim";
/// Edge kind id registered in the canvas `VisualRegistry`.
pub(crate) const EDGE_KIND: &str = "usd.wire";

// Layout constants (world units). A node is a fixed card; ranks march right,
// rows march down. Wide enough to fit a prim leaf name + type label.
const NODE_W: f32 = 160.0;
const NODE_H: f32 = 72.0;
const COL_SPACING: f32 = 280.0;
const ROW_SPACING: f32 = 120.0;
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
    pub type_name: String,
    pub is_body: bool,
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
        let is_body = view.has_api_schema(&p, "PhysicsRigidBodyAPI");
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
            type_name,
            is_body,
            inputs,
            outputs,
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

    // Dataflow layering: rank(sink) ≥ rank(source) + 1, relaxed to a fixed
    // point. `n` passes converge any DAG; the final clamp bounds cycles.
    let mut rank: Vec<i32> = vec![0; n];
    for _ in 0..n {
        let mut changed = false;
        for w in &wires {
            if w.kind != WireKind::Dataflow {
                continue;
            }
            let (s, t) = (index[&w.source_path], index[&w.target_path]);
            if rank[t] < rank[s] + 1 {
                rank[t] = rank[s] + 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let max_rank = (n as i32 - 1).max(0);
    for r in rank.iter_mut() {
        *r = (*r).min(max_rank);
    }

    // Position: column by rank, row by order-within-rank.
    let mut rows_per_rank: HashMap<i32, u32> = HashMap::new();
    let mut positions: Vec<Pos> = vec![Pos::default(); n];
    for i in 0..n {
        let r = rank[i];
        let row = *rows_per_rank.get(&r).unwrap_or(&0);
        rows_per_rank.insert(r, row + 1);
        positions[i] = Pos::new(
            MARGIN + r as f32 * COL_SPACING,
            MARGIN + row as f32 * ROW_SPACING,
        );
    }

    let mut scene = Scene::new();
    let mut node_ids = Vec::with_capacity(n);
    for i in 0..n {
        let node = &relevant[i];
        let rect = Rect::from_min_size(positions[i], NODE_W, NODE_H);
        let mut ports: Vec<Port> = Vec::new();

        let ins: Vec<&String> = in_ports[i].iter().collect();
        for (k, name) in ins.iter().enumerate() {
            ports.push(Port {
                id: PortId::new((*name).clone()),
                local_offset: Pos::new(0.0, port_y(k, ins.len())),
                kind: "input".into(),
            });
        }
        let outs: Vec<&String> = out_ports[i].iter().collect();
        for (k, name) in outs.iter().enumerate() {
            ports.push(Port {
                id: PortId::new((*name).clone()),
                local_offset: Pos::new(NODE_W, port_y(k, outs.len())),
                kind: "output".into(),
            });
        }
        // Hidden joint anchors — `~jr` (right) sources a joint edge, `~jl` (left)
        // sinks it. Prefixed `~` so the visual skips painting them. Present on
        // every node so any joint edge resolves.
        ports.push(Port {
            id: PortId::new("~jr"),
            local_offset: Pos::new(NODE_W, NODE_H * 0.5),
            kind: "joint".into(),
        });
        ports.push(Port {
            id: PortId::new("~jl"),
            local_offset: Pos::new(0.0, NODE_H * 0.5),
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
            label: leaf,
            origin: Some(node.path.clone()),
            resizable: false,
            visual_rect: None,
        });
        node_ids.push(id);
    }

    for w in &wires {
        let (s, t) = (index[&w.source_path], index[&w.target_path]);
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
            waypoints: Vec::new(),
            waypoints_authored: false,
        });
    }

    let _ = empty_node_data; // (kept in scope for symmetry with scene.rs helpers)
    scene
}

/// Even vertical distribution of `count` ports down a `NODE_H`-tall edge:
/// port `k` sits at `H·(k+1)/(count+1)`.
fn port_y(k: usize, count: usize) -> f32 {
    NODE_H * (k as f32 + 1.0) / (count as f32 + 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prim(path: &str, ins: &[&str], outs: &[&str], is_body: bool) -> PrimNode {
        PrimNode {
            path: path.to_string(),
            type_name: "Xform".to_string(),
            is_body,
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
            amp.ports.iter().any(|p| p.id.as_str() == "signal"
                && p.kind.as_str() == "input"),
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
        let nodes = vec![
            prim("/A", &[], &[], true),
            prim("/B", &[], &[], true),
        ];
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
}
