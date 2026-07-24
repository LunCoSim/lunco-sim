//! Canvas diagram operation builders + appliers.
//!
//! Translates user gestures (`lunco_canvas::CanvasEvent`) into
//! `ModelicaOp` writes against the active `Document`. Includes the
//! optimistic apply path used to keep the canvas in sync with the
//! document's per-class AST after every edit, plus the auto-arrange
//! observer that re-projects then runs the auto-layout pass.

use bevy::prelude::*;

use crate::document::ModelicaOp;
use crate::pretty::{self, Placement};
use crate::state::ModelicaDocumentRegistry;
use crate::ui::commands::AutoArrangeDiagram;

use super::coords::{canvas_to_modelica, ModelicaPos};
use super::projection::projection_relevant_source_hash;
use super::{active_doc_from_world, active_doc_from_world_ctx, CanvasDiagramState, IconNodeData};
use crate::model_tabs_types::TabRenderContext;

/// Read the active tab id from `TabRenderContext`. `None` outside a
/// panel render call (observers, off-render systems); call sites that
/// pair this with `get_for_render` correctly fall back to first-tab
/// semantics in that case.
#[cfg(feature = "ui")]
fn render_tab_id_ctx(ctx: &lunco_workbench::PanelCtx) -> Option<crate::model_tabs_types::TabId> {
    ctx.resource::<TabRenderContext>().and_then(|c| c.tab_id)
}

/// Resolve `(document id, editing class name)` for the current tab.
/// `PanelCtx` reader used by the canvas render path.
#[cfg(feature = "ui")]
pub(super) fn resolve_doc_context(
    ctx: &lunco_workbench::PanelCtx,
) -> (Option<lunco_doc::DocumentId>, Option<String>) {
    // Active doc from the Workspace session; the per-doc Index
    // is read as a display-cache fallback when the registry AST hasn't
    // caught up yet. Both paths are optional — the caller tolerates
    // `(None, None)` by deferring.
    let Some(doc_id) = ctx
        .resource::<lunco_workspace::WorkspaceResource>()
        .and_then(|w| w.active_document)
    else {
        return (None, None);
    };
    // Class resolution priority — must match `compile_model`'s logic
    // and `active_class_for_doc` so the canvas's *edit* target lines
    // up with what compile / projection consider authoritative:
    //   1. drilled-in pin (user explicitly navigated into a class)
    //   2. first non-package class via `extract_model_name_from_ast`
    //   3. the per-doc Index (display cache)
    //
    // The previous `s.classes.keys().next()` returned the IndexMap's
    // first key, which for a multi-class file wrapped in a `package`
    // (AnnotatedRocketStage, every MSL example, …) is the *package*
    // wrapper. Adding a component to a package corrupts the file —
    // packages can only contain classes, not components.
    let drilled_in = crate::sim_default::drilled_class_for_doc_ctx(ctx, doc_id);
    let class = drilled_in
        .or_else(|| {
            ctx.resource::<ModelicaDocumentRegistry>()
                .and_then(|r| r.host(doc_id))
                .and_then(|h| {
                    h.document()
                        .strict_ast()
                        .and_then(|ast| crate::ast_extract::extract_model_name_from_ast(&ast))
                })
        })
        .or_else(|| crate::state::detected_name_for_ctx(ctx, doc_id));
    (Some(doc_id), class)
}

// Thin wrapper so existing call sites keep their shape. The real
// conversion lives in `super::coords::canvas_min_to_modelica_center`.

/// Translate canvas scene events into ModelicaOps. Needs a brief
/// read-only borrow of the scene (to look up edge endpoints); the
/// caller runs it inside its own borrow scope.
#[cfg(feature = "ui")]
pub(super) fn build_ops_from_events(
    ctx: &lunco_workbench::PanelCtx,
    state: &CanvasDiagramState,
    events: &[lunco_canvas::SceneEvent],
    class: &str,
) -> Vec<ModelicaOp> {
    use lunco_canvas::SceneEvent;
    let active_doc = active_doc_from_world_ctx(ctx);
    let tab = render_tab_id_ctx(ctx);
    let scene = &state.get_for_render(tab, active_doc).canvas.scene;
    let mut ops: Vec<ModelicaOp> = Vec::new();

    for ev in events {
        match ev {
            SceneEvent::NodeMoved { id, new_min, .. } => {
                let Some(node) = scene.node(*id) else {
                    continue;
                };
                // Plot tiles are vendor-annotation rows in
                // `Diagram(graphics)`, not component placements. They
                // round-trip through `SetPlotNodeExtent` keyed by
                // signal path; the on-screen rect is taken straight
                // from `node.rect` (canvas world coords match the
                // Modelica diagram coord system). Identification:
                // origin format is `"plot:<idx>:<signal>"` — split
                // off the signal to use as the op key.
                if node.kind == lunco_viz::kinds::canvas_plot_node::PLOT_NODE_KIND {
                    let signal = node
                        .origin
                        .as_deref()
                        .and_then(|o| o.strip_prefix("plot:"))
                        .and_then(|rest| rest.split_once(':').map(|(_, s)| s.to_string()))
                        .or_else(|| {
                            // Fallback for legacy / scratch plot
                            // nodes whose origin isn't in the source
                            // form yet — pull the signal out of the
                            // node's `data` payload.
                            node.data
                                .downcast_ref::<lunco_viz::kinds::canvas_plot_node::PlotNodeData>()
                                .map(|d| d.signal_path.clone())
                        });
                    let Some(signal_path) = signal.filter(|s| !s.is_empty()) else {
                        continue;
                    };
                    let w = node.rect.width().max(1.0);
                    let h = node.rect.height().max(1.0);
                    ops.push(ModelicaOp::SetPlotNodeExtent {
                        class: class.to_string(),
                        signal_path,
                        x1: new_min.x,
                        y1: new_min.y,
                        x2: new_min.x + w,
                        y2: new_min.y + h,
                    });
                    continue;
                }
                if node.kind == crate::ui::text_node::TEXT_NODE_KIND {
                    let Some(idx) = node
                        .origin
                        .as_deref()
                        .and_then(|o| o.strip_prefix("text:"))
                        .and_then(|n| n.parse::<usize>().ok())
                    else {
                        continue;
                    };
                    let w = node.rect.width().max(1.0);
                    let h = node.rect.height().max(1.0);
                    // Canvas → Modelica: negate Y so the source
                    // sees +Y up and the round-trip is stable
                    // (re-projection emits the same screen rect).
                    ops.push(ModelicaOp::SetDiagramTextExtent {
                        class: class.to_string(),
                        index: idx,
                        x1: new_min.x,
                        y1: -new_min.y,
                        x2: new_min.x + w,
                        y2: -(new_min.y + h),
                    });
                    continue;
                }
                // The `origin` we set during projection carries the
                // Modelica instance name. Skip if missing (shouldn't
                // happen — projection always sets it).
                let Some(name) = node.origin.clone() else {
                    continue;
                };
                // Use the node's actual icon extent — `Placement::at`
                // hardcodes 20×20, which silently shrinks (or grows)
                // every dragged component back to the default size on
                // re-projection. Read the live `node.rect` instead so
                // the new placement preserves whatever size the icon
                // already has on screen (canvas world coords are 1:1
                // with Modelica units, just Y-flipped).
                let icon_w = node.rect.width().max(1.0);
                let icon_h = node.rect.height().max(1.0);
                let m = super::coords::canvas_min_to_modelica_center(*new_min, icon_w, icon_h);
                ops.push(ModelicaOp::SetPlacement {
                    class: class.to_string(),
                    name,
                    placement: Placement {
                        x: m.x,
                        y: m.y,
                        width: icon_w,
                        height: icon_h,
                    },
                });
                // Rubber-band: incident *authored* wires get their
                // port-side interior waypoint slid by the node's
                // delta so hand-routed bends still meet the moved
                // port. Auto-routed wires (`waypoints_authored=false`)
                // skip — re-projection re-runs A* with the new port
                // positions, no annotation persistence needed.
                let old_min = match ev {
                    SceneEvent::NodeMoved { old_min, .. } => *old_min,
                    _ => continue,
                };
                let dx = new_min.x - old_min.x;
                let dy = new_min.y - old_min.y;
                if dx.abs() < 0.001 && dy.abs() < 0.001 {
                    // no-op move (snap-back) — skip
                } else {
                    let moved_node_id = *id;
                    let incident: Vec<(lunco_canvas::EdgeId, bool, bool)> = scene
                        .edges()
                        .filter_map(|(eid, e)| {
                            if !e.waypoints_authored || e.waypoints.is_empty() {
                                return None;
                            }
                            if e.kind.as_str() != "modelica.connection" {
                                return None;
                            }
                            let is_from = e.from.node == moved_node_id;
                            let is_to = e.to.node == moved_node_id;
                            if is_from || is_to {
                                Some((*eid, is_from, is_to))
                            } else {
                                None
                            }
                        })
                        .collect();
                    for (eid, is_from, is_to) in incident {
                        let Some(edge) = scene.edge(eid) else {
                            continue;
                        };
                        let mut pts = edge.waypoints.clone();
                        if is_from && !pts.is_empty() {
                            pts[0] = lunco_canvas::Pos::new(pts[0].x + dx, pts[0].y + dy);
                        }
                        if is_to && !pts.is_empty() {
                            let last = pts.len() - 1;
                            pts[last] = lunco_canvas::Pos::new(pts[last].x + dx, pts[last].y + dy);
                        }
                        let Some(from_node) = scene.node(edge.from.node) else {
                            continue;
                        };
                        let Some(to_node) = scene.node(edge.to.node) else {
                            continue;
                        };
                        let Some(from_instance) = from_node.origin.clone() else {
                            continue;
                        };
                        let Some(to_instance) = to_node.origin.clone() else {
                            continue;
                        };
                        let modelica_points: Vec<(f32, f32)> =
                            pts.iter().map(|p| (p.x, -p.y)).collect();
                        ops.push(ModelicaOp::SetConnectionLine {
                            class: class.to_string(),
                            from: pretty::PortRef::new(&from_instance, edge.from.port.as_str()),
                            to: pretty::PortRef::new(&to_instance, edge.to.port.as_str()),
                            points: modelica_points,
                        });
                    }
                }
            }
            SceneEvent::NodeResized { id, new_rect, .. } => {
                let Some(node) = scene.node(*id) else {
                    continue;
                };
                if node.kind == lunco_viz::kinds::canvas_plot_node::PLOT_NODE_KIND {
                    let signal = node
                        .origin
                        .as_deref()
                        .and_then(|o| o.strip_prefix("plot:"))
                        .and_then(|rest| rest.split_once(':').map(|(_, s)| s.to_string()))
                        .or_else(|| {
                            node.data
                                .downcast_ref::<lunco_viz::kinds::canvas_plot_node::PlotNodeData>()
                                .map(|d| d.signal_path.clone())
                        });
                    let Some(signal_path) = signal.filter(|s| !s.is_empty()) else {
                        continue;
                    };
                    ops.push(ModelicaOp::SetPlotNodeExtent {
                        class: class.to_string(),
                        signal_path,
                        x1: new_rect.min.x,
                        y1: new_rect.min.y,
                        x2: new_rect.max.x,
                        y2: new_rect.max.y,
                    });
                    continue;
                }
                if node.kind == crate::ui::text_node::TEXT_NODE_KIND {
                    let Some(idx) = node
                        .origin
                        .as_deref()
                        .and_then(|o| o.strip_prefix("text:"))
                        .and_then(|n| n.parse::<usize>().ok())
                    else {
                        continue;
                    };
                    ops.push(ModelicaOp::SetDiagramTextExtent {
                        class: class.to_string(),
                        index: idx,
                        x1: new_rect.min.x,
                        y1: -new_rect.min.y,
                        x2: new_rect.max.x,
                        y2: -new_rect.max.y,
                    });
                    continue;
                }
                // Component icon resize → `SetPlacement` keeping
                // the node's centre fixed but adopting the new
                // width/height. Lets users tighten oversized library
                // icons on the canvas without writing source by hand.
                let Some(name) = node.origin.clone() else {
                    continue;
                };
                let w = new_rect.width().max(1.0);
                let h = new_rect.height().max(1.0);
                let m = super::coords::canvas_min_to_modelica_center(new_rect.min, w, h);
                ops.push(ModelicaOp::SetPlacement {
                    class: class.to_string(),
                    name,
                    placement: Placement {
                        x: m.x,
                        y: m.y,
                        width: w,
                        height: h,
                    },
                });
            }
            SceneEvent::EdgeCreated { from, to, points } => {
                // Resolve canvas port refs → Modelica (instance,
                // port) pairs via node.origin + port.id.
                let Some(from_node) = scene.node(from.node) else {
                    continue;
                };
                let Some(to_node) = scene.node(to.node) else {
                    continue;
                };
                let Some(from_instance) = from_node.origin.clone() else {
                    continue;
                };
                let Some(to_instance) = to_node.origin.clone() else {
                    continue;
                };
                // Click-to-bend during creation → annotation(Line(...))
                // with the captured points (Y-flipped into Modelica
                // coords). Empty list = quick drag, no annotation,
                // domain layer auto-routes.
                let line = if points.is_empty() {
                    None
                } else {
                    Some(pretty::Line {
                        points: points.iter().map(|p| (p.x, -p.y)).collect(),
                    })
                };
                ops.push(ModelicaOp::AddConnection {
                    class: class.to_string(),
                    eq: pretty::ConnectEquation {
                        from: pretty::PortRef::new(&from_instance, from.port.as_str()),
                        to: pretty::PortRef::new(&to_instance, to.port.as_str()),
                        line,
                    },
                });
            }
            SceneEvent::EdgeDeleted { id } => {
                if let Some(op) = op_remove_edge_inner(scene, *id, class) {
                    ops.push(op);
                }
            }
            SceneEvent::NodeDeleted { id, orphaned_edges } => {
                // Orphan edge RemoveConnection ops must go in
                // BEFORE the RemoveComponent so rumoca still sees
                // the edges while resolving the connect(...) spans.
                for eid in orphaned_edges {
                    if let Some(op) = op_remove_edge_inner(scene, *eid, class) {
                        ops.push(op);
                    }
                }
                if let Some(op) = op_remove_node_inner(scene, *id, class) {
                    ops.push(op);
                }
            }
            SceneEvent::EdgeWaypointsChanged { id, points } => {
                let Some(edge) = scene.edge(*id) else {
                    continue;
                };
                if edge.kind.as_str() != "modelica.connection" {
                    continue;
                }
                let Some(from_node) = scene.node(edge.from.node) else {
                    continue;
                };
                let Some(to_node) = scene.node(edge.to.node) else {
                    continue;
                };
                let Some(from_instance) = from_node.origin.clone() else {
                    continue;
                };
                let Some(to_instance) = to_node.origin.clone() else {
                    continue;
                };
                // Canvas Y is +down; Modelica diagram Y is +up. Flip
                // so the round-trip back through `extract_line_points`
                // lands at the same canvas positions.
                let modelica_points: Vec<(f32, f32)> = points.iter().map(|p| (p.x, -p.y)).collect();
                ops.push(ModelicaOp::SetConnectionLine {
                    class: class.to_string(),
                    from: pretty::PortRef::new(&from_instance, edge.from.port.as_str()),
                    to: pretty::PortRef::new(&to_instance, edge.to.port.as_str()),
                    points: modelica_points,
                });
            }
            _ => {}
        }
    }
    ops
}

/// `(instance_name, type_label)` for a node, pulled from the scene's
/// `label` + `data.type`. Empty strings when the node is gone.
#[cfg(feature = "ui")]
pub(super) fn component_headers(
    ctx: &lunco_workbench::PanelCtx,
    state: &CanvasDiagramState,
    id: lunco_canvas::NodeId,
) -> (String, String) {
    let active_doc = active_doc_from_world_ctx(ctx);
    let tab = render_tab_id_ctx(ctx);
    let Some(node) = state.get_for_render(tab, active_doc).canvas.scene.node(id) else {
        return (String::new(), String::new());
    };
    let instance = node.label.clone();
    let type_name = node
        .data
        .downcast_ref::<IconNodeData>()
        .map(|d| d.qualified_type.clone())
        .unwrap_or_default();
    (instance, type_name)
}

/// Pick the next free instance name in `scene` for `comp`. First
/// letter of the short class name + smallest unused integer (`R1`,
/// `R2`, …). Walks `scene.nodes()` directly so the choice respects
/// nodes the user has just optimistically synthesised but that
/// haven't yet round-tripped through the AST.
pub(super) fn pick_add_instance_name(
    comp: &crate::index::ClassEntry,
    scene: &lunco_canvas::Scene,
) -> String {
    let prefix = comp.name.chars().next().unwrap_or('X').to_ascii_uppercase();
    let mut n: u32 = 1;
    loop {
        let candidate = format!("{prefix}{n}");
        let taken = scene
            .nodes()
            .any(|(_, node)| node.origin.as_deref() == Some(candidate.as_str()));
        if !taken {
            return candidate;
        }
        n += 1;
    }
}

/// Build an `AddComponent` op at a world-space position with a
/// caller-chosen instance name. Carries the component's default
/// parameter values and a `Placement` annotation so the new node
/// lands at the right spot in both the source and any downstream
/// re-projection.
pub(super) fn op_add_component_with_name(
    comp: &crate::index::ClassEntry,
    instance_name: &str,
    at_world: lunco_canvas::Pos,
    class: &str,
) -> ModelicaOp {
    let ModelicaPos { x: mx, y: my } = canvas_to_modelica(at_world);
    ModelicaOp::AddComponent {
        class: class.to_string(),
        decl: pretty::ComponentDecl {
            type_name: comp.name.clone(),
            name: instance_name.to_string(),
            modifications: comp
                .parameters
                .iter()
                .filter(|p| !p.default.is_empty())
                .map(|p| (p.name.clone(), p.default.clone()))
                .collect(),
            placement: Some(Placement::at(mx, my)),
        },
    }
}

// `synthesize_msl_node` — optimistic-scene helper — was deleted in
// A.4. Used to insert a Node into the canvas scene the same frame the
// op fired, ahead of the projection re-derivation. After A.2 the
// AST-canonical apply path is fast (no debounced reparse during
// apply) and the projection system runs every tick, so the next
// frame's projection picks up the new gen and renders the same node
// — no perceptible latency. Removing the optimistic path also kills
// a small drift class: the optimistic Node and the projected Node
// could disagree on port layout / icon rendering until the projector
// caught up.

#[cfg(feature = "ui")]
pub(super) fn op_remove_component(
    ctx: &lunco_workbench::PanelCtx,
    state: &CanvasDiagramState,
    id: lunco_canvas::NodeId,
    class: &str,
) -> Option<ModelicaOp> {
    let active_doc = active_doc_from_world_ctx(ctx);
    let tab = render_tab_id_ctx(ctx);
    op_remove_node_inner(
        &state.get_for_render(tab, active_doc).canvas.scene,
        id,
        class,
    )
}

#[cfg(feature = "ui")]
pub(super) fn op_remove_edge(
    ctx: &lunco_workbench::PanelCtx,
    state: &CanvasDiagramState,
    id: lunco_canvas::EdgeId,
    class: &str,
) -> Option<ModelicaOp> {
    let active_doc = active_doc_from_world_ctx(ctx);
    let tab = render_tab_id_ctx(ctx);
    op_remove_edge_inner(
        &state.get_for_render(tab, active_doc).canvas.scene,
        id,
        class,
    )
}

pub(super) fn op_remove_node_inner(
    scene: &lunco_canvas::Scene,
    id: lunco_canvas::NodeId,
    class: &str,
) -> Option<ModelicaOp> {
    let node = scene.node(id)?;
    // Plot tiles delete via `RemovePlotNode` keyed by signal path,
    // not `RemoveComponent` which targets a Modelica component
    // declaration. Same dispatch shape as the move handler above.
    if node.kind == lunco_viz::kinds::canvas_plot_node::PLOT_NODE_KIND {
        let signal_path = node
            .origin
            .as_deref()
            .and_then(|o| o.strip_prefix("plot:"))
            .and_then(|rest| rest.split_once(':').map(|(_, s)| s.to_string()))
            .or_else(|| {
                node.data
                    .downcast_ref::<lunco_viz::kinds::canvas_plot_node::PlotNodeData>()
                    .map(|d| d.signal_path.clone())
            })
            .filter(|s| !s.is_empty())?;
        return Some(ModelicaOp::RemovePlotNode {
            class: class.to_string(),
            signal_path,
        });
    }
    if node.kind == crate::ui::text_node::TEXT_NODE_KIND {
        let idx = node
            .origin
            .as_deref()
            .and_then(|o| o.strip_prefix("text:"))
            .and_then(|n| n.parse::<usize>().ok())?;
        return Some(ModelicaOp::RemoveDiagramText {
            class: class.to_string(),
            index: idx,
        });
    }
    let name = node.origin.clone()?;
    Some(ModelicaOp::RemoveComponent {
        class: class.to_string(),
        name,
    })
}

pub(super) fn op_remove_edge_inner(
    scene: &lunco_canvas::Scene,
    id: lunco_canvas::EdgeId,
    class: &str,
) -> Option<ModelicaOp> {
    let edge = scene.edge(id)?;
    let from_node = scene.node(edge.from.node)?;
    let to_node = scene.node(edge.to.node)?;
    let from_instance = from_node.origin.clone()?;
    let to_instance = to_node.origin.clone()?;
    Some(ModelicaOp::RemoveConnection {
        class: class.to_string(),
        from: pretty::PortRef::new(&from_instance, edge.from.port.as_str()),
        to: pretty::PortRef::new(&to_instance, edge.to.port.as_str()),
    })
}

/// Apply a batch of ops against the bound document. Ops that fail
/// (e.g. RemoveComponent when the instance isn't actually in source
/// — shouldn't happen, but defence in depth) are logged and
/// skipped. After success the doc's generation bumps, which the
/// next frame picks up via `last_seen_gen` and re-projects.
/// Public re-export of the canvas's op applier so reflect-registered
/// commands (`MoveComponent`, etc.) can dispatch the same SetPlacement
/// pipeline the mouse drag uses — keeps undo/redo + source rewriting
/// consistent across UI-driven and API-driven edits.
pub fn apply_ops_public(world: &mut World, doc_id: lunco_doc::DocumentId, ops: Vec<ModelicaOp>) {
    apply_ops(
        world,
        doc_id,
        ops,
        lunco_twin_journal::AuthorTag::local_user(),
    );
}

// The author-specifying batch entry point (used by the API / agent bridges)
// is now the egui-free `crate::doc_ops::apply_ops_as` — callers that don't
// need the canvas flourishes apply there directly.

pub(super) fn apply_ops(
    world: &mut World,
    doc_id: lunco_doc::DocumentId,
    ops: Vec<ModelicaOp>,
    author: lunco_twin_journal::AuthorTag,
) {
    let t_start = web_time::Instant::now();
    // Auto-pin every tab pointing at this doc — VS Code semantics:
    // any edit to a preview tab promotes it to a permanent tab so
    // the next browser click doesn't replace it. Cheap (one
    // HashMap walk over open tabs).
    world
        .resource_mut::<crate::model_tabs::ModelTabs>()
        .pin_all_for_doc(doc_id);
    // Stamp the post-apply window so the canvas frame logger
    // captures every subsequent frame's timing for ~2 seconds.
    if let Ok(mut guard) = crate::ui::panels::canvas_diagram::panel::util::LAST_APPLY_AT.lock() {
        *guard = Some(t_start);
    }
    // Stamp the GLOBAL frame-time probe so every Bevy Update tick
    // (not just canvas render) gets logged for the next 5 seconds —
    // catches main-thread blocks anywhere in the schedule.
    crate::frame_time_probe_stamp_edit(world);
    // Capture before the core consumes `ops`: did this batch include a
    // drag-style op whose visual was already applied to the scene?
    // See the `canvas_acked_gen` block below.
    let any_optimistic_visual = ops
        .iter()
        .any(|op| matches!(op, ModelicaOp::SetPlacement { .. }));

    // Core mutation (egui-free): batch deferral + per-op kernel + canonical
    // journal + read-only banner + mark_changed + MSL preload. Shared with
    // the API / headless path (`crate::doc_ops::apply_ops_as`) so the canvas
    // and the API apply ops identically. Returns whether anything applied
    // synchronously (false on full-batch deferral or all-no-op).
    let any_applied = crate::doc_ops::apply_ops_as(world, doc_id, ops, author);
    if !any_applied {
        bevy::log::info!(
            "[CanvasDiagram] apply_ops (no-op / deferred) total={:.1}ms",
            t_start.elapsed().as_secs_f64() * 1000.0
        );
        return;
    }

    let t_mirror_start = web_time::Instant::now();
    // Mirror the post-edit source back to the registry-by-doc lookup
    // so every other panel (code editor, breadcrumb, inspector)
    // that reads the cached source sees the update immediately —
    // the code editor doesn't watch the registry directly; it
    // reads the `Arc<str>` on `open_model`.
    let fresh = world
        .get_resource::<ModelicaDocumentRegistry>()
        .and_then(|r| r.host(doc_id))
        .map(|h| {
            (
                h.document().source_arc(),
                <crate::document::ModelicaDocument as lunco_doc::Document>::generation(
                    h.document(),
                ),
            )
        });
    if let Some((src, new_gen)) = fresh {
        // readers go through the registry directly. `src` is still
        // used for the projection-relevant hash below.
        // Canvas-originated edits have *already* mutated the scene
        // before reaching apply_ops (drag moved the node; menu Add
        // synthesised a node prior to dispatch). Acknowledging the
        // new generation tells the project gate "the scene already
        // reflects this state — don't re-project" — but **only for
        // the tab the user actually edited**. Other tabs viewing
        // the same doc (splits) have stale scenes and *do* need to
        // reproject; leaving their `last_seen_gen` untouched lets
        // the gen-advance check fire on their next render.
        let new_hash = projection_relevant_source_hash(&*src);
        let editing_tab = world
            .resource::<crate::model_tabs_types::TabRenderContext>()
            .tab_id;
        // Ack the gen on the editing tab so its render loop won't
        // re-project (it already shows the new state). Sibling tabs
        // viewing the same `(doc, drilled)` are kept in sync via
        // [`apply_event_to_sibling_scene`] — replayed by the canvas
        // panel right after `canvas.ui()` returns events. Mutations
        // that don't have a SceneEvent equivalent (menu add /
        // remove, palette drop) fall through to gen-advance on the
        // sibling's next render, which reprojects from the
        // freshly-rewritten source.
        // The previous code unconditionally acked the new gen on the
        // editing tab to suppress projection — the assumption being
        // that the canvas had already optimistically synthesised the
        // affected node/edge before dispatch. That optimistic-synth
        // path (`synthesize_msl_node`) was deleted; menu-add /
        // palette-drop / context-menu ops now produce *no* same-frame
        // scene change. Acking the gen here was telling the projection
        // gate "scene already matches new state" — a lie — and the
        // gate then skipped projection forever, leaving the user
        // staring at an empty canvas after adding components.
        //
        // We only ack for ops whose visual is genuinely already
        // applied by the time apply_ops returns. Today the canvas
        // only does true optimistic update for drag (`SetPlacement`
        // adjusts the scene's node rect in-place before the op
        // dispatches). Everything else needs the projector to rebuild
        // the scene from the new AST — let `gen_advanced` fire.
        if any_optimistic_visual {
            if let Some(mut state) = world.get_resource_mut::<CanvasDiagramState>() {
                if let Some(tab_id) = editing_tab {
                    let docstate = state.get_mut_for_tab(tab_id, doc_id);
                    docstate.canvas_acked_gen = new_gen;
                    docstate.last_seen_gen = new_gen;
                    docstate.last_seen_source_hash = new_hash;
                } else {
                    let docstate = state.get_mut(Some(doc_id));
                    docstate.canvas_acked_gen = new_gen;
                    docstate.last_seen_gen = new_gen;
                    docstate.last_seen_source_hash = new_hash;
                }
            }
        }
        // Non-drag ops: leave `last_seen_gen`, `canvas_acked_gen`,
        // AND `last_seen_source_hash` untouched. The next canvas
        // render sees `gen_advanced=true` plus a fresh source hash
        // that no longer matches the stored one, and the projection
        // gate fires a reprojection. Updating the hash here (a
        // previous mistake) made the hash-skip check pass falsely
        // and the canvas never re-rendered the new component.
        let _ = new_hash;
    }

    let mirror_ms = t_mirror_start.elapsed().as_secs_f64() * 1000.0;

    // Wake egui. Without this, the canvas panel's `render` only
    // fires on the next input event, so the projection task that
    // would materialise the new component sits idle for whatever
    // egui's reactive sleep happens to be (~2 s in practice). The
    // panel's render pass is what *spawns* the projection task and
    // *polls* the in-flight task — both gated on render running.
    // Pinging every EguiContext component (one per window) brings
    // the next paint within ~16ms, the projection cycle wakes up,
    // and the right-click → component-appears latency drops from
    // multi-second to imperceptible.
    let t_repaint_start = web_time::Instant::now();
    let mut q = world.query::<&mut bevy_egui::EguiContext>();
    for mut ctx in q.iter_mut(world) {
        ctx.get_mut().request_repaint();
    }
    let repaint_ms = t_repaint_start.elapsed().as_secs_f64() * 1000.0;

    bevy::log::debug!(
        "[CanvasDiagram] apply_ops timing: mirror={:.1}ms repaint={:.1}ms total={:.1}ms",
        mirror_ms,
        repaint_ms,
        t_start.elapsed().as_secs_f64() * 1000.0
    );
}

/// Observer for [`crate::ui::commands::AutoArrangeDiagram`].
///
/// Assigns every component of the active class a grid position from
/// the current [`crate::ui::panels::canvas_projection::DiagramAutoLayoutSettings`]
/// `arrange_*` parameters and emits a batch of `SetPlacement` ops.
///
/// Iterates the canvas scene (not the AST) so the order matches what
/// the user sees. Each op is separately undo-able via Ctrl+Z.
#[lunco_core::on_command(AutoArrangeDiagram)]
pub fn on_auto_arrange_diagram(trigger: On<AutoArrangeDiagram>, mut commands: Commands) {
    let raw = trigger.event().doc;
    // Observers can't take `&mut World` in Bevy 0.18. Defer the real
    // work to an exclusive command — same mutations, just queued to
    // run at the next command-flush boundary.
    commands.queue(move |world: &mut World| {
        // `doc = 0` = API / script default = "the tab the user is
        // looking at right now". Resolve from the registry-by-doc lookup
        // so the LunCo API can fire the command without tracking ids.
        let doc_id = if raw.is_unassigned() {
            match active_doc_from_world(world) {
                Some(d) => d,
                None => {
                    bevy::log::warn!("[CanvasDiagram] Auto-Arrange: no active doc");
                    return;
                }
            }
        } else {
            raw
        };
        auto_arrange_now(world, doc_id);
    });
}

pub(super) fn auto_arrange_now(world: &mut World, doc_id: lunco_doc::DocumentId) {
    let Some(class) = active_class_for_doc(world, doc_id) else {
        return;
    };
    let layout = world
        .get_resource::<crate::ui::panels::canvas_projection::DiagramAutoLayoutSettings>()
        .cloned()
        .unwrap_or_default();
    // Capture each node's `origin` (Modelica instance name) AND
    // its existing rect size so Auto-Arrange can preserve per-node
    // extents — the prior `Placement::at` form squashed every icon
    // back to the default 20×20, undoing the user's authored sizes.
    let mut named_with_size: Vec<(String, f32, f32)> = {
        let Some(state) = world.get_resource::<CanvasDiagramState>() else {
            return;
        };
        let docstate = state.get(Some(doc_id));
        docstate
            .canvas
            .scene
            .nodes()
            .filter_map(|(_, n)| {
                let origin = n.origin.clone()?;
                Some((origin, n.rect.width().max(1.0), n.rect.height().max(1.0)))
            })
            .collect()
    };
    // Stable sort + dedup by name: the original `dedup()` only
    // removed adjacent duplicates, which the unsorted scene order
    // didn't guarantee.
    named_with_size.sort_by(|a, b| a.0.cmp(&b.0));
    named_with_size.dedup_by(|a, b| a.0 == b.0);
    if named_with_size.is_empty() {
        return;
    }

    let cols = layout.cols.max(1);
    let dx = layout.spacing_x;
    let dy = layout.spacing_y;
    let stagger = dx * layout.row_stagger;
    let ops: Vec<ModelicaOp> = named_with_size
        .into_iter()
        .enumerate()
        .map(|(idx, (name, w, h))| {
            let row = idx / cols;
            let col = idx % cols;
            let row_shift = if row % 2 == 1 { stagger } else { 0.0 };
            // Canvas world coords (+Y down). Convert to Modelica
            // centre (+Y up) via the shared helper so the ops emit
            // the same coord frame a drag would.
            let wx = col as f32 * dx + row_shift;
            let wy = row as f32 * dy;
            let m =
                super::coords::canvas_min_to_modelica_center(lunco_canvas::Pos::new(wx, wy), w, h);
            ModelicaOp::SetPlacement {
                class: class.clone(),
                name,
                placement: Placement {
                    x: m.x,
                    y: m.y,
                    width: w,
                    height: h,
                },
            }
        })
        .collect();
    if ops.is_empty() {
        return;
    }
    bevy::log::info!(
        "[CanvasDiagram] Auto-Arrange: emitting {} SetPlacement ops",
        ops.len()
    );
    #[cfg(feature = "lunco-api")]
    crate::api::trigger_apply_ops(world, doc_id, ops);
    #[cfg(not(feature = "lunco-api"))]
    apply_ops_public(world, doc_id, ops);
}

/// Resolve the active class name for an Auto-Arrange target. Prefers
/// the drilled-in class name (for MSL drill-in tabs); falls back to
/// the open document's detected model name.
pub fn active_class_for_doc(world: &mut World, doc_id: lunco_doc::DocumentId) -> Option<String> {
    // `DrilledInClassNames` cache).
    if let Some(c) = crate::sim_default::drilled_class_for_doc(world, doc_id) {
        return Some(c);
    }
    crate::state::detected_name_for(world, doc_id)
}

/// `PanelCtx` sibling of [`active_class_for_doc`] — same precedence,
/// reading resources through the capability-narrowed panel context.
pub fn active_class_for_doc_ctx(
    ctx: &lunco_workbench::PanelCtx,
    doc_id: lunco_doc::DocumentId,
) -> Option<String> {
    if let Some(c) = crate::sim_default::drilled_class_for_doc_ctx(ctx, doc_id) {
        return Some(c);
    }
    crate::state::detected_name_for_ctx(ctx, doc_id)
}
