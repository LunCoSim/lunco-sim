//! Off-thread projection: Modelica AST → canvas `Scene`.
//!
//! `project_scene` walks a `VisualDiagram` and produces a fully-laid-
//! out `lunco_canvas::Scene` plus a `DiagramNodeId → CanvasNodeId`
//! map. `recover_edges_from_ast` salvages connections from a recovery
//! parse when the strict AST omits them. `projection_relevant_source_hash`
//! is the cache key. `ProjectionTask` carries the in-flight Bevy
//! task handle so the panel can poll it for completion.

use std::collections::HashMap;

use bevy_egui::egui;
use lunco_canvas::{
    Edge as CanvasEdge, Node as CanvasNode, NodeId as CanvasNodeId, Port as CanvasPort,
    PortId as CanvasPortId, PortRef, Pos as CanvasPos, Rect as CanvasRect, Scene,
};

use crate::visual_diagram::{DiagramNodeId, VisualDiagram};

use super::edge::{port_edge_dir, ConnectionEdgeData, PortDir};
use super::node::IconNodeData;
use super::port::{port_fallback_offset_for_size, port_kind_str, resolve_port_icons};
use super::si_unit_suffix;

/// Regex-scan `connect(a.b, c.d);` patterns in `source` and add
/// matching edges to `diagram`. Skips equations whose components
/// aren't in the diagram (missing nodes stay visually missing) or
/// that already exist as edges (keyed by unordered endpoint pair).
///
/// Deliberately permissive: doesn't validate port existence, doesn't
/// care about the line-continuation form, doesn't consult
/// annotations. "Text says A.x ↔ B.y; show a line between A and B".
pub(super) fn recover_edges_from_ast(
    ast: &rumoca_compile::parsing::ast::StoredDefinition,
    diagram: &mut VisualDiagram,
) {
    // Walk every class's `Equation::Connect` and add any edges the
    // primary diagram-build path missed (top-level connectors that
    // the component-graph builder skips because they aren't
    // sub-components). Iterating the AST that the projection task
    // already holds — no second parse.
    //
    // AST-as-source-of-truth: the previous implementation called
    // `parse_to_ast(source, "recover.mo")` here, re-parsing the same
    // bytes the projection task already received. Removed — re-parse
    // inside a projection task is a bug.

    // Build (instance_name → DiagramNodeId) index once per call.
    let index: HashMap<String, DiagramNodeId> = diagram
        .nodes
        .iter()
        .map(|n| (n.instance_name.clone(), n.id))
        .collect();

    // Track existing edges as unordered pairs so we don't double-
    // add when the AST path already caught a connection.
    let existing: std::collections::HashSet<((String, String), (String, String))> = diagram
        .edges
        .iter()
        .map(|e| {
            let a = (
                diagram
                    .get_node(e.source_node)
                    .map(|n| n.instance_name.clone())
                    .unwrap_or_default(),
                e.source_port.clone(),
            );
            let b = (
                diagram
                    .get_node(e.target_node)
                    .map(|n| n.instance_name.clone())
                    .unwrap_or_default(),
                e.target_port.clone(),
            );
            // Canonicalise to min/max so (A.x, B.y) == (B.y, A.x).
            if a <= b {
                (a, b)
            } else {
                (b, a)
            }
        })
        .collect();

    fn walk(
        class: &rumoca_compile::parsing::ast::ClassDef,
        diagram: &mut VisualDiagram,
        index: &HashMap<String, DiagramNodeId>,
        existing: &std::collections::HashSet<((String, String), (String, String))>,
    ) {
        use rumoca_compile::parsing::ast::Equation;
        for eq in &class.equations {
            let Equation::Connect { lhs, rhs, .. } = eq else {
                continue;
            };
            // Only handle 2+ part references (`inst.port[.subport]`);
            // single-part bare-connector connects are caught by the
            // primary AST path that builds the component graph.
            let (src_comp, src_port) = match lhs.parts.as_slice() {
                [first, rest @ ..] if !rest.is_empty() => (
                    first.ident.text.to_string(),
                    rest.iter()
                        .map(|p| p.ident.text.as_ref())
                        .collect::<Vec<_>>()
                        .join("."),
                ),
                _ => continue,
            };
            let (tgt_comp, tgt_port) = match rhs.parts.as_slice() {
                [first, rest @ ..] if !rest.is_empty() => (
                    first.ident.text.to_string(),
                    rest.iter()
                        .map(|p| p.ident.text.as_ref())
                        .collect::<Vec<_>>()
                        .join("."),
                ),
                _ => continue,
            };
            let (Some(&src_id), Some(&tgt_id)) = (index.get(&src_comp), index.get(&tgt_comp))
            else {
                continue;
            };
            let pair = {
                let a = (src_comp.clone(), src_port.clone());
                let b = (tgt_comp.clone(), tgt_port.clone());
                if a <= b {
                    (a, b)
                } else {
                    (b, a)
                }
            };
            if existing.contains(&pair) {
                continue;
            }
            diagram.add_edge(src_id, src_port, tgt_id, tgt_port);
        }
        for nested in class.classes.values() {
            walk(nested, diagram, index, existing);
        }
    }
    for class in ast.classes.values() {
        walk(class, diagram, &index, &existing);
    }
}

pub(super) fn project_scene(
    diagram: &VisualDiagram,
) -> (Scene, HashMap<DiagramNodeId, CanvasNodeId>) {
    let mut scene = Scene::new();
    let mut id_map: HashMap<DiagramNodeId, CanvasNodeId> = HashMap::new();

    for node in &diagram.nodes {
        let cid = scene.alloc_node_id();
        id_map.insert(node.id, cid);

        // Ports: map Modelica (-100..100, +Y up) to local icon box
        // (0..ICON_W, 0..ICON_H, +Y down). If a port has no
        // annotated position (both x and y at 0 — the default when
        // the component class didn't declare one), fall back to
        // distributing around the icon's edges: alternating left
        // and right for the classic two-terminal electrical shape,
        // extending up for more ports. Matches what OMEdit does
        // when Placement annotations are missing.
        // The single source of truth for this node's icon-local →
        // canvas-world transform. Built once by the importer from the
        // Placement, applied uniformly here for the rect, ports, and
        // (eventually) the icon body.
        let xform = node.icon_transform;

        // Bounding rect = AABB of the icon's local extent
        // ({{-100,-100},{100,100}} per MLS default) under the
        // transform. Honours rotation naturally (a 45°-rotated icon
        // gets a larger axis-aligned rect than its unrotated form).
        let ((min_wx, min_wy), (max_wx, max_wy)) = xform.local_aabb(-100.0, -100.0, 100.0, 100.0);
        let icon_w_local = (max_wx - min_wx).max(4.0);
        let icon_h_local = (max_wy - min_wy).max(4.0);

        let n_ports = node.component_def.ports.len();
        let ports: Vec<CanvasPort> = node
            .component_def
            .ports
            .iter()
            .enumerate()
            .map(|(i, p)| {
                // Port positions in icon-local Modelica coords go
                // through the same transform — no per-feature
                // mirror/rotate branches, just one matrix multiply.
                // The result is in canvas world; we convert to
                // icon-local *screen* coords (relative to the rect's
                // top-left) since `CanvasPort.local_offset` is icon-
                // local, not world.
                let (wx, wy) = if p.x == 0.0 && p.y == 0.0 {
                    // Fallback layout: distribute around the rect.
                    // Already in icon-local screen coords — convert
                    // to world by adding the rect's top-left.
                    let (fx, fy) =
                        port_fallback_offset_for_size(i, n_ports, icon_w_local, icon_h_local);
                    (min_wx + fx, min_wy + fy)
                } else {
                    xform.apply(p.x, p.y)
                };
                let lx = wx - min_wx;
                let ly = wy - min_wy;
                CanvasPort {
                    id: CanvasPortId::new(p.name.clone()),
                    local_offset: CanvasPos::new(lx, ly),
                    // AST-derived causality classification as a short
                    // string (`"input"` / `"output"` / `"acausal"`) —
                    // the canvas renderer's port-shape match reads
                    // this directly, so MSL naming conventions are
                    // no longer needed to pick the right shape.
                    kind: port_kind_str(p.kind).into(),
                }
            })
            .collect();

        scene.insert_node(CanvasNode {
            id: cid,
            rect: CanvasRect::from_min_size(
                CanvasPos::new(min_wx, min_wy),
                icon_w_local,
                icon_h_local,
            ),
            kind: "modelica.icon".into(),
            data: std::sync::Arc::new(IconNodeData {
                qualified_type: node.component_def.name.clone(),
                icon_only: crate::ui::loaded_classes::is_icon_only_class(&node.component_def.name),
                expandable_connector: node.component_def.is_expandable_connector(),
                icon_graphics: node.component_def.icon.clone(),
                diagram_graphics: if matches!(
                    node.component_def.kind,
                    crate::index::ClassKind::Connector
                        | crate::index::ClassKind::ExpandableConnector
                ) {
                    node.component_def.diagram_graphics.clone()
                } else {
                    None
                },
                rotation_deg: node.icon_transform.rotation_deg,
                mirror_x: node.icon_transform.mirror_x,
                mirror_y: node.icon_transform.mirror_y,
                instance_name: node.instance_name.clone(),
                parameters: node
                    .component_def
                    .parameters
                    .iter()
                    .map(|p| {
                        let v = node
                            .parameter_values
                            .get(&p.name)
                            .cloned()
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| p.default.clone());
                        let value = match si_unit_suffix(&p.param_type) {
                            Some(unit) if !v.is_empty() => format!("{v} {unit}"),
                            _ => v,
                        };
                        (p.name.clone(), value)
                    })
                    .collect(),
                port_connector_paths: node
                    .component_def
                    .ports
                    .iter()
                    .map(|p| {
                        (
                            p.name.clone(),
                            p.msl_path.clone(),
                            p.size_x,
                            p.size_y,
                            p.rotation_deg,
                        )
                    })
                    .collect(),
                port_connector_icons: resolve_port_icons(
                    &node.component_def.name,
                    &node.component_def.ports,
                ),
                is_conditional: node.is_conditional,
            }),
            ports,
            label: node.instance_name.clone(),
            origin: Some(node.instance_name.clone()),
            // Modelica icons are sized by their `Icon` annotation; a
            // user-driven resize would desync from the source. Plot /
            // dashboard nodes opt into resize via the default `true`.
            resizable: false,
            // Tight halo follows the visible graphics, not the full
            // -100..100 placement frame. Modelica icons commonly only
            // fill ~50 % of their frame (e.g. Tank's body uses
            // -50..50), so a placement-rect halo leaves big empty
            // bands inside the selection box. Apply the same
            // `xform` that mapped icon-local → world to the graphics
            // bbox so rotation / mirror are honoured.
            visual_rect: node
                .component_def
                .icon
                .as_ref()
                .and_then(|icon| icon.graphics_bbox())
                .map(|e| {
                    let ((vx0, vy0), (vx1, vy1)) = xform.local_aabb(
                        e.p1.x as f32,
                        e.p1.y as f32,
                        e.p2.x as f32,
                        e.p2.y as f32,
                    );
                    CanvasRect::from_min_max(CanvasPos::new(vx0, vy0), CanvasPos::new(vx1, vy1))
                }),
        });
    }

    for edge in &diagram.edges {
        let Some(src_cid) = id_map.get(&edge.source_node) else {
            continue;
        };
        let Some(tgt_cid) = id_map.get(&edge.target_node) else {
            continue;
        };

        // Look up the source / target port definitions so we can
        // bake connector type + edge-side direction into the edge's
        // data. The visual reads both for colour selection and
        // port-direction stubs without needing world access.
        let src_node = diagram.nodes.iter().find(|n| n.id == edge.source_node);
        let tgt_node = diagram.nodes.iter().find(|n| n.id == edge.target_node);
        // Port lookup falls back to the head segment so qualified
        // sub-port references like `flange.phi` (from
        // `recover_edges_from_source`) still resolve to the
        // outer `flange` PortDef. Without this, every recovered
        // edge with a sub-port lost its colour + stub direction
        // because the find() returned None.
        let find_port = |defs: &[crate::visual_diagram::PortDef],
                         name: &str|
         -> Option<crate::visual_diagram::PortDef> {
            if let Some(p) = defs.iter().find(|p| p.name == name) {
                return Some(p.clone());
            }
            let head = name.split('.').next().unwrap_or(name);
            defs.iter().find(|p| p.name == head).cloned()
        };
        let src_port_def =
            src_node.and_then(|n| find_port(&n.component_def.ports, &edge.source_port));
        let tgt_port_def =
            tgt_node.and_then(|n| find_port(&n.component_def.ports, &edge.target_port));
        let connector_type = src_port_def
            .as_ref()
            .map(|p| p.connector_type.clone())
            .unwrap_or_default();
        // Wire color sourced from the connector class's Icon
        // (populated by the projector for both local & MSL types).
        // Falls back to `null` so the edge factory uses the leaf-name
        // palette in `wire_color_for`. The source-level
        // `Line(color={r,g,b})` annotation wins over the
        // connector-derived colour so the wire properties dialog's
        // colour pick lands immediately in the visual.
        let icon_color = edge.color.or_else(|| {
            src_port_def
                .as_ref()
                .and_then(|p| p.color)
                .or_else(|| tgt_port_def.as_ref().and_then(|p| p.color))
        });
        // Stub direction = which edge the port sits on in *screen*
        // space. Apply the owning instance's transform's linear part
        // (no translation — directions don't have a position). One
        // matrix multiply per port replaces the previous four
        // per-feature branches (mirror_x, mirror_y, rotate_x, …).
        let from_dir = match (src_node, src_port_def.as_ref()) {
            (Some(n), Some(p)) => {
                let (dx, dy) = n.icon_transform.apply_dir(p.x, p.y);
                port_edge_dir(dx, dy)
            }
            _ => PortDir::None,
        };
        let to_dir = match (tgt_node, tgt_port_def.as_ref()) {
            (Some(n), Some(p)) => {
                let (dx, dy) = n.icon_transform.apply_dir(p.x, p.y);
                port_edge_dir(dx, dy)
            }
            _ => PortDir::None,
        };

        let eid = scene.alloc_edge_id();

        // Auto-route waypoints when the source has no authored
        // `Line(points={...})` annotation. A* on a 4-unit grid with
        // a bend penalty + obstacle inflation produces clean L / Z /
        // wrap-around routes that the per-frame Z-bend heuristic
        // can't manage. Authored waypoints win — preserves user
        // intent on hand-routed connections.
        let waypoints_authored = !edge.waypoints.is_empty();
        let waypoints_world: Vec<CanvasPos> = if !edge.waypoints.is_empty() {
            edge.waypoints
                .iter()
                .map(|&(x, y)| CanvasPos::new(x, -y))
                .collect()
        } else {
            // World endpoints: port world position via owning
            // node's transform.
            let src_world = src_node.and_then(|n| {
                src_port_def
                    .as_ref()
                    .map(|p| n.icon_transform.apply(p.x, p.y))
            });
            let tgt_world = tgt_node.and_then(|n| {
                tgt_port_def
                    .as_ref()
                    .map(|p| n.icon_transform.apply(p.x, p.y))
            });
            match (src_world, tgt_world) {
                (Some(s), Some(t)) => {
                    let from_out = from_dir.outward();
                    let to_out = to_dir.outward();
                    let obstacles: Vec<crate::ui::wire_router::Obstacle> = scene
                        .nodes()
                        .filter(|(id, _)| **id != *src_cid && **id != *tgt_cid)
                        .filter(|(_, n)| n.kind.as_str() == "modelica.icon")
                        .map(|(_, n)| {
                            let r = n.visual_rect.unwrap_or(n.rect);
                            crate::ui::wire_router::Obstacle {
                                min_x: r.min.x,
                                min_y: r.min.y,
                                max_x: r.max.x,
                                max_y: r.max.y,
                            }
                        })
                        .collect();
                    // grid 4 / bend 80 / clearance 2: bend penalty
                    // is 20× the step cost so A* very strongly prefers
                    // 1- or 2-bend routes over wrappy multi-bend ones.
                    // Earlier value (16) was tied with relatively short
                    // detours, so the green Tank.mass_out wire took a
                    // 4-bend wrap around the engine when a 2-bend
                    // route over the top was available.
                    let pts = crate::ui::wire_router::route(
                        s, from_out, t, to_out, &obstacles, 4.0, 80.0, 2.0,
                    );
                    // Strip endpoints — `waypoints_world` carries
                    // *interior* bends only; the renderer prepends /
                    // appends the actual port positions.
                    if pts.len() >= 2 {
                        pts[1..pts.len() - 1]
                            .iter()
                            .map(|&(x, y)| CanvasPos::new(x, y))
                            .collect()
                    } else {
                        Vec::new()
                    }
                }
                _ => Vec::new(),
            }
        };
        scene.insert_edge(CanvasEdge {
            id: eid,
            from: PortRef {
                node: *src_cid,
                port: CanvasPortId::new(edge.source_port.clone()),
            },
            to: PortRef {
                node: *tgt_cid,
                port: CanvasPortId::new(edge.target_port.clone()),
            },
            kind: "modelica.connection".into(),
            // Mirror the interior polyline (authored or auto-routed)
            // into the scene's first-class waypoints field so the
            // canvas tool can hit-test + mutate it during drag without
            // reaching into per-domain edge data. Renderers (both egui
            // and Vello) now read this field directly, so there's no
            // frozen copy to drift out of sync. `waypoints_authored`
            // distinguishes user-authored bends (from source
            // annotation) from auto-router output so rubber-band on
            // node move only persists what the user actually authored.
            waypoints: waypoints_world,
            waypoints_authored,
            data: std::sync::Arc::new(ConnectionEdgeData {
                connector_type: connector_type.clone(),
                from_dir,
                to_dir,
                icon_color: icon_color.map(|[r, g, b]| egui::Color32::from_rgb(r, g, b)),
                source_path: src_node
                    .map(|n| format!("{}.{}", n.instance_name, edge.source_port))
                    .unwrap_or_default(),
                target_path: tgt_node
                    .map(|n| format!("{}.{}", n.instance_name, edge.target_port))
                    .unwrap_or_default(),
                kind: src_port_def
                    .as_ref()
                    .map(|p| p.kind)
                    .unwrap_or(crate::visual_diagram::PortKind::Acausal),
                flow_vars: src_port_def
                    .as_ref()
                    .map(|p| p.flow_vars.clone())
                    .unwrap_or_default(),
                smooth_bezier: edge.smooth_bezier,
                // Modelica default thickness is 0.25; we expose it as
                // a multiplier on the renderer's base stroke width so
                // the existing zoom/causality math stays untouched.
                // Source-level overrides map ~1.0…8.0 ⇒ visually
                // distinguishable wires; clamp to keep extreme
                // values from making wires unreadable.
                thickness_scale: edge
                    .thickness
                    .map(|t| (t / 0.25).clamp(0.5, 6.0))
                    .unwrap_or(1.0),
            }),
            origin: None,
        });
        // ── DIAG: per-projection record of what flow_vars resolved
        // for each edge. Lets us correlate "edge X had empty
        // flow_vars on first projection but populated after re-
        // projection on node move" with port-resolution behaviour.
        let src_path_dbg = src_node
            .map(|n| format!("{}.{}", n.instance_name, edge.source_port))
            .unwrap_or_default();
        let tgt_path_dbg = tgt_node
            .map(|n| format!("{}.{}", n.instance_name, edge.target_port))
            .unwrap_or_default();
        let src_port_ports: Vec<String> = src_node
            .map(|n| {
                n.component_def
                    .ports
                    .iter()
                    .map(|p| p.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        let resolved_flow_vars: Vec<String> = src_port_def
            .as_ref()
            .map(|p| p.flow_vars.iter().map(|f| f.name.clone()).collect())
            .unwrap_or_default();
        // Per-edge diagnostic — `debug!`, not `info!`, so it doesn't spam the
        // browser console on every projection. Console I/O is a real cost on
        // wasm and this fires once per edge × every reproject. `RUST_LOG=debug`
        // brings it back.
        bevy::log::debug!(
            "[proj-diag] edge {src_path_dbg} -> {tgt_path_dbg} \
             src_port_def_found={} src_node_ports={src_port_ports:?} \
             flow_vars={resolved_flow_vars:?} connector_type={:?}",
            src_port_def.is_some(),
            connector_type,
        );
    }

    (scene, id_map)
}

/// Hash the *projection-relevant* slice of source — collapses runs
/// of whitespace into single spaces and drops `//` line comments
/// and `/* … */` block comments. String literals are preserved
/// (they include filenames in `Bitmap(fileName=...)` annotations,
/// which DO affect rendering).
///
/// Used by the cheap "edit-class skip": when the document
/// generation bumps but this hash hasn't moved, the edit was a
/// comment / blank-line / parameter-default tweak that doesn't
/// change the projected scene topology — skip the projection task
/// entirely. Catches the bulk of the typing-latency regressions on
/// large MSL files.
///
/// Note: false negatives (edits that DO change projection but
/// produce the same hash) are impossible — the hash domain
/// includes every glyph in components / equations / annotations.
/// False positives (edits that DON'T change projection but bump
/// the hash) are fine: we just over-project, same as before.
pub(super) fn projection_relevant_source_hash(source: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut chars = source.chars().peekable();
    let mut in_string = false;
    let mut last_was_ws = true;
    while let Some(c) = chars.next() {
        if in_string {
            c.hash(&mut h);
            if c == '"' {
                in_string = false;
            }
            continue;
        }
        if c == '/' {
            match chars.peek() {
                Some('/') => {
                    chars.next();
                    while let Some(&n) = chars.peek() {
                        if n == '\n' {
                            break;
                        }
                        chars.next();
                    }
                    continue;
                }
                Some('*') => {
                    chars.next();
                    while let Some(c2) = chars.next() {
                        if c2 == '*' && chars.peek() == Some(&'/') {
                            chars.next();
                            break;
                        }
                    }
                    continue;
                }
                _ => {}
            }
        }
        if c == '"' {
            in_string = true;
            c.hash(&mut h);
            last_was_ws = false;
            continue;
        }
        if c.is_whitespace() {
            if !last_was_ws {
                ' '.hash(&mut h);
                last_was_ws = true;
            }
            continue;
        }
        c.hash(&mut h);
        last_was_ws = false;
    }
    h.finish()
}

/// Running projection task + the generation that spawned it, so the
/// poll loop can tell whether we've moved on since and should drop a
/// stale result. The owning doc is implicit: each task lives on that
/// doc's [`crate::ui::panels::canvas_diagram::CanvasDocState`].
///
/// # Cancellation
///
/// Bevy tasks can't be preempted — "cancel" is cooperative. We
/// give the task a shared `AtomicBool` and a deadline; it polls
/// them at phase boundaries (build → edges recovery → project)
/// and returns an empty `Scene` if either fires. The poll loop
/// drops the handle when the deadline elapses even if the task
/// hasn't noticed yet — the pool runs it to completion but nobody
/// reads the result.
///
/// Two independent "stop" signals:
///
/// - **`cancel`** — flipped to `true` explicitly (user hits
///   cancel, new generation supersedes, tab closed, etc.).
/// - **`deadline`** — wall-clock elapsed > configured max. Reads
///   live via `spawned_at.elapsed() > deadline`.
pub struct ProjectionTask {
    pub gen_at_spawn: u64,
    /// Document the projection was spawned for. Captured at spawn
    /// because the world's "active document" can drift while the
    /// task runs off-thread (user switches tabs, duplicates a doc,
    /// closes one). On completion the canvas swap layer uses *this*
    /// doc id — not the live active doc — to tag source-backed plot
    /// tiles' `PlotBinding::Doc` with the right document, otherwise
    /// they'd resolve to whichever sim is active in another tab.
    pub doc_at_spawn: lunco_doc::DocumentId,
    /// Drill-in target the projection was spawned for. Compared
    /// against `CanvasDocState::last_seen_target` on completion so
    /// the UI knows which target produced the rendered scene.
    pub target_at_spawn: Option<String>,
    pub spawned_at: web_time::Instant,
    pub deadline: std::time::Duration,
    pub cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The pool task. Spawned via
    /// [`lunco_workbench::tracked_task::spawn_tracked_cancellable`], so a
    /// `StatusBus` `BusyHandle` lives inside the future and clears the
    /// bus entry on completion, panic, supersede, or drop.
    pub task: lunco_workbench::tracked_task::TrackedTask<Scene>,
    /// Projection-relevant source hash captured at spawn time.
    /// Stashed onto `CanvasDocState::last_seen_source_hash` when the
    /// task completes — used by the next gen-bump check to skip
    /// reprojection on no-op edits (whitespace, comments).
    pub source_hash: u64,
}
