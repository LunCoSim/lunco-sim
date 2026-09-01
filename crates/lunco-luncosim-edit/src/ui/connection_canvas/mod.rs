//! USD **connection canvas** — a node-graph view of a scene's wiring.
//!
//! A second projector over the generic `lunco-canvas` substrate (the Modelica
//! diagram is the first). It reads the live composed USD stage and renders each
//! wiring-relevant prim as a node and each co-sim connection / physics joint as
//! an edge; dragging port-to-port authors a `SetConnection`, and Delete clears
//! a wire or removes a prim — all through the journaled USD command path.
//!
//! # Pipeline
//!
//! ```text
//!   CanonicalStage (composed USD)
//!         │  collect_graph()          (read complete authored topology)
//!         ▼
//!   Vec<PrimNode> + Vec<Wire>
//!         │  project_schema()         (explicit authored schema view)
//!         │  build_scene()            (pure: relevance filter + layering)
//!         ▼
//!   lunco_canvas::Scene → Canvas → egui
//!         ▲                 │
//!         └── SceneEvent ───┘  → UsdOp (SetConnection / RemovePrim) → ApplyUsdOps
//! ```
//!
//! The producer runs on the **main thread** (the stage is `!Send`) and rebuilds
//! only when the projected topology changes (hash-gated), so pan / zoom / drag
//! and selection survive between structural edits. Node *positions* are
//! session-only for v1 — a structural edit re-lays-out; persisting a
//! `lunco:canvasPos` is a follow-up.

mod projection;
mod visuals;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_canvas::{Canvas, EdgeId, NodeId, PortRef, Scene, SceneEvent, VisualRegistry};
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelScrollPolicy, PanelSlot};

use lunco_doc::DocumentId;
use lunco_modelica::ui::commands::FocusDocumentByName;
use lunco_usd::document::{LayerId, UsdOp};
use lunco_usd::ui::viewport::{UsdPreviewId, UsdPreviewSession, UsdViewportState};
use lunco_usd_bevy::{CanonicalStages, UsdPrimPath, UsdStageAsset};

use projection::{
    build_scene, collect_graph, project_schema, schema_roots, PrimNode, UsdPrimNodeData,
    UsdWireData, Wire, WireKind, EDGE_KIND, NODE_KIND,
};

pub use lunco_usd::ui::USD_CONNECTION_CANVAS_PANEL_ID as USD_CANVAS_PANEL_ID;

/// Build the visual registry for the USD canvas — one node kind, one edge kind.
fn build_registry() -> VisualRegistry {
    let mut reg = VisualRegistry::new();
    reg.register_node_kind(NODE_KIND, |data: &lunco_canvas::NodeData| match data
        .downcast_ref::<UsdPrimNodeData>()
    {
        Some(d) => visuals::node_visual(d),
        None => visuals::UsdPrimNodeVisual {
            type_name: String::new(),
            is_body: false,
        },
    });
    reg.register_edge_kind(EDGE_KIND, |data: &lunco_canvas::NodeData| match data
        .downcast_ref::<UsdWireData>()
    {
        Some(d) => visuals::edge_visual(d),
        None => visuals::UsdWireVisual {
            kind: WireKind::Dataflow,
        },
    });
    reg
}

/// One preview lease's canvas plus the bindings the producer resolves so the
/// write-back path knows which document and authored layer to use.
pub struct UsdCanvasSessionState {
    canvas: Canvas,
    /// Stage currently projected — used to detect a scene swap.
    stage_id: Option<AssetId<UsdStageAsset>>,
    /// Editable document backing `stage_id`, if resolvable. A preview lease is
    /// created only for an open document, so edits are never kept as a local
    /// canvas-only mutation.
    doc: Option<DocumentId>,
    /// Authored layer selected by this preview lease.
    edit_target: Option<LayerId>,
    /// Document generation captured with the preview projection.
    generation: u64,
    /// Hash of the last projected topology; a rebuild is skipped while it holds
    /// so interaction (pan/zoom/drag/select) isn't stomped every frame.
    topo_hash: u64,
    built: bool,
    /// Frame-to-fit request. Set by the producer on a stage swap; consumed by
    /// the panel's first render, which alone knows the real widget size (the
    /// producer only has a nominal guess).
    needs_fit: bool,
    /// Complete collected topology retained so changing the active authored
    /// schema root is a presentation operation, not a stage reload.
    source_nodes: Vec<PrimNode>,
    source_wires: Vec<Wire>,
    schema_roots: Vec<String>,
    active_schema_root: Option<String>,
}

impl Default for UsdCanvasSessionState {
    fn default() -> Self {
        let mut canvas = Canvas::new(build_registry());
        // USD scenes can contain many composed participants.  The generic
        // canvas minimum (0.25) is intentionally comfortable for hand-built
        // diagrams, but it prevents a composed flight stack from ever fitting
        // in one frame.  The connection view owns this scale policy because
        // it knows the scene is a document-sized graph, not a small sketch.
        canvas.viewport.config.zoom_min = 0.04;
        Self {
            canvas,
            stage_id: None,
            doc: None,
            edit_target: None,
            generation: 0,
            topo_hash: 0,
            built: false,
            needs_fit: false,
            source_nodes: Vec::new(),
            source_wires: Vec::new(),
            schema_roots: Vec::new(),
            active_schema_root: None,
        }
    }
}

impl UsdCanvasSessionState {
    fn clear(&mut self) {
        self.canvas.scene = Scene::default();
        self.canvas.selection.clear();
        self.stage_id = None;
        self.doc = None;
        self.edit_target = None;
        self.generation = 0;
        self.topo_hash = 0;
        self.built = false;
        self.needs_fit = false;
        self.source_nodes.clear();
        self.source_wires.clear();
        self.schema_roots.clear();
        self.active_schema_root = None;
    }
}

/// Session-keyed connection canvases. Canvas interaction state (pan, zoom,
/// graph selection, and chosen schema root) belongs to its preview lease.
#[derive(Resource, Default)]
pub struct UsdCanvasState {
    sessions: HashMap<UsdPreviewId, UsdCanvasSessionState>,
}

/// Order-stable hash of the projected topology (paths + connectors + wires).
/// Node positions and selection are intentionally excluded so a drag doesn't
/// trigger a re-layout.
fn topology_hash(nodes: &[projection::PrimNode], wires: &[projection::Wire]) -> u64 {
    let mut keys: Vec<String> = Vec::with_capacity(nodes.len() + wires.len());
    for n in nodes {
        keys.push(format!(
            "N|{}|{}|{}|{}|{}|{:?}|{:?}|{}|{}",
            n.path,
            n.is_body,
            n.schema_root,
            n.schema_node,
            n.display_name.as_deref().unwrap_or_default(),
            n.schema_column,
            n.schema_row,
            n.inputs.join(","),
            n.outputs.join(",")
        ));
    }
    for w in wires {
        keys.push(format!(
            "W|{:?}|{}.{}|{}.{}",
            w.kind, w.source_path, w.source_conn, w.target_path, w.target_conn
        ));
    }
    keys.sort();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    keys.hash(&mut h);
    h.finish()
}

/// View-model producer (WP-8): reads each open preview's composed stage and
/// rebuilds its canvas scene when the topology changes. Runs on the main thread
/// because `StageView` is `!Send`.
pub fn produce_usd_canvas(
    q: Query<(Entity, &UsdPrimPath)>,
    q_parents: Query<&ChildOf>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<CanonicalStages>,
    viewport_state: Option<Res<UsdViewportState>>,
    mut views: ResMut<UsdCanvasState>,
) {
    let Some(viewport) = viewport_state.as_deref() else {
        views.sessions.clear();
        return;
    };
    let open: std::collections::HashSet<_> = viewport.sessions().map(|s| s.id()).collect();
    views.sessions.retain(|preview, _| open.contains(preview));
    for session in viewport.sessions() {
        let state = views
            .sessions
            .entry(session.id())
            .or_insert_with(UsdCanvasSessionState::default);
        produce_usd_canvas_session(session, &q, &q_parents, &stages, &mut canonical, state);
    }
}

fn produce_usd_canvas_session(
    session: &UsdPreviewSession,
    q: &Query<(Entity, &UsdPrimPath)>,
    q_parents: &Query<&ChildOf>,
    stages: &Assets<UsdStageAsset>,
    canonical: &mut CanonicalStages,
    state: &mut UsdCanvasSessionState,
) {
    let doc = session.doc();
    let handle = session.stage_handle().clone();
    let preview_root = session.scene_root();
    let stage_id = handle.id();

    // A lease replacement invalidates the complete interaction model before a
    // new stage becomes available; a loading document cannot show or edit the
    // previous document's graph.
    if state.doc != Some(doc) || state.stage_id != Some(stage_id) {
        state.clear();
    }
    state.edit_target = Some(session.edit_target().clone());
    state.generation = session.projected_generation();

    let is_preview_entity =
        |entity: Entity| crate::ui::is_editor_preview_entity(entity, preview_root, q_parents);
    if canonical.get(stage_id).is_none() {
        if let Some(recipe) = stages.get(&handle).and_then(|a| a.recipe.clone()) {
            canonical.get_or_build(stage_id, &recipe);
        }
    }
    let Some(cs) = canonical.get(stage_id) else {
        return;
    };
    let prim_paths: Vec<String> = q
        .iter()
        .filter(|(entity, p)| p.stage_handle.id() == stage_id && is_preview_entity(*entity))
        .map(|(_, p)| p.path.clone())
        .collect();
    let view = cs.view();
    let (source_nodes, source_wires) = collect_graph(&view, &prim_paths);
    let roots = schema_roots(&source_nodes);
    let active_root = state
        .active_schema_root
        .as_ref()
        .filter(|root| roots.contains(root))
        .cloned();
    let (nodes, wires) = active_root
        .as_deref()
        .map(|root| project_schema(source_nodes.clone(), source_wires.clone(), root))
        .unwrap_or_default();
    let hash = topology_hash(&nodes, &wires);

    if state.built && state.stage_id == Some(stage_id) && state.topo_hash == hash {
        state.source_nodes = source_nodes;
        state.source_wires = source_wires;
        state.schema_roots = roots;
        state.active_schema_root = active_root;
        return;
    }

    let scene = build_scene(nodes, wires);
    let bounds = scene.bounds();
    bevy::log::debug!(
        "[usd-canvas] preview {} rebuilt: {} prim entities -> {} nodes, {} edges",
        session.id().0,
        prim_paths.len(),
        scene.node_count(),
        scene.edge_count()
    );
    state.canvas.scene = scene;
    state.canvas.selection.clear();
    state.source_nodes = source_nodes;
    state.source_wires = source_wires;
    state.schema_roots = roots;
    state.active_schema_root = active_root;
    state.topo_hash = hash;
    state.stage_id = Some(stage_id);
    state.built = true;
    state.doc = Some(doc);
    if bounds.is_some() {
        state.needs_fit = true;
    }
}

/// Wake the Editor connection graph when its explicit preview document or
/// the composed USD stage changes. A missing preview clears the graph through
/// the producer instead of leaving the previous document visible.
pub fn editor_canvas_changed(
    viewport: Option<Res<UsdViewportState>>,
    revision: Res<lunco_usd_bevy::UsdStageRevision>,
) -> bool {
    viewport.is_some_and(|state| state.is_changed()) || revision.is_changed()
}

// ─── Write-back: SceneEvent → UsdOp ─────────────────────────────────────────

/// A dataflow edge's sink, snapshotted before `Canvas::ui` may delete it — the
/// info needed to clear that wire's `inputs:<c>.connect`.
struct EdgeSink {
    prim: String,
    connector: String,
}

/// Resolve an edge's sink prim + connector from its `to` endpoint (dataflow
/// edges are authored source-output → sink-input, so `to` is always the sink).
fn edge_sink(scene: &Scene, id: EdgeId) -> Option<EdgeSink> {
    let e = scene.edge(id)?;
    // Joints have no dataflow connection to clear.
    if e.data
        .downcast_ref::<UsdWireData>()
        .map(|d| d.kind != WireKind::Dataflow)
        .unwrap_or(true)
    {
        return None;
    }
    let prim = scene.node(e.to.node)?.origin.clone()?;
    Some(EdgeSink {
        prim,
        connector: e.to.port.as_str().to_string(),
    })
}

/// Classify an `EdgeCreated`'s two endpoints into (source-output, sink-input)
/// by port kind, then author the sink's `inputs:<c>.connect`.
fn connect_op(scene: &Scene, from: &PortRef, to: &PortRef, edit_target: &LayerId) -> Option<UsdOp> {
    let kind = |pr: &PortRef| -> Option<&str> {
        scene
            .node(pr.node)?
            .ports
            .iter()
            .find(|p| p.id == pr.port)
            .map(|p| p.kind.as_str())
    };
    let (source, sink) = match (kind(from)?, kind(to)?) {
        ("output", "input") => (from, to),
        ("input", "output") => (to, from),
        // Same-side or joint anchors — not a dataflow wire the user can author.
        _ => return None,
    };
    let source_prim = scene.node(source.node)?.origin.clone()?;
    let sink_prim = scene.node(sink.node)?.origin.clone()?;
    let sink_conn = sink.port.as_str();
    let source_conn = source.port.as_str();
    Some(UsdOp::SetConnection {
        edit_target: edit_target.clone(),
        path: sink_prim,
        name: format!("inputs:{sink_conn}"),
        // Co-sim ports are authored `float` (the convention rewire reads).
        type_name: "float".to_string(),
        sources: vec![format!("{source_prim}.outputs:{source_conn}")],
    })
}

/// Turn one frame's scene events into USD ops. `node_origin` / `edge_sinks` are
/// snapshotted before `Canvas::ui` mutates the scene (deleted nodes/edges are
/// gone from `scene` by the time this runs); `EdgeCreated` reads the still-valid
/// post-`ui` scene for port kinds.
fn build_ops(
    scene: &Scene,
    node_origin: &HashMap<NodeId, String>,
    edge_sinks: &HashMap<EdgeId, EdgeSink>,
    events: &[SceneEvent],
    edit_target: &LayerId,
) -> Vec<UsdOp> {
    let mut ops = Vec::new();
    for ev in events {
        match ev {
            SceneEvent::EdgeCreated { from, to, .. } => {
                if let Some(op) = connect_op(scene, from, to, edit_target) {
                    ops.push(op);
                }
            }
            SceneEvent::EdgeDeleted { id } => {
                if let Some(sink) = edge_sinks.get(id) {
                    ops.push(UsdOp::SetConnection {
                        edit_target: edit_target.clone(),
                        path: sink.prim.clone(),
                        name: format!("inputs:{}", sink.connector),
                        type_name: "float".to_string(),
                        sources: Vec::new(), // clear the wire
                    });
                }
            }
            SceneEvent::NodeDeleted { id, orphaned_edges } => {
                // Clear any dataflow wire that fed this prim, then remove it.
                for eid in orphaned_edges {
                    if let Some(sink) = edge_sinks.get(eid) {
                        ops.push(UsdOp::SetConnection {
                            edit_target: edit_target.clone(),
                            path: sink.prim.clone(),
                            name: format!("inputs:{}", sink.connector),
                            type_name: "float".to_string(),
                            sources: Vec::new(),
                        });
                    }
                }
                if let Some(path) = node_origin.get(id) {
                    ops.push(UsdOp::RemovePrim {
                        edit_target: edit_target.clone(),
                        path: path.clone(),
                    });
                }
            }
            _ => {}
        }
    }
    ops
}

// ─── Panel ──────────────────────────────────────────────────────────────────

pub struct UsdCanvasPanel;

impl Panel for UsdCanvasPanel {
    fn id(&self) -> PanelId {
        USD_CANVAS_PANEL_ID
    }
    fn title(&self) -> String {
        "Connections".into()
    }
    fn menu_group(&self) -> lunco_workbench::PanelMenuGroup {
        lunco_workbench::PanelMenuGroup::Scene
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Center
    }
    fn scroll_policy(&self) -> PanelScrollPolicy {
        PanelScrollPolicy::SelfManaged
    }
    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let focused_preview = ctx
            .resource::<UsdViewportState>()
            .and_then(|viewport| viewport.focused_preview_id());
        ctx.resource_scope::<UsdCanvasState, ()>(|ctx, views| {
            let Some(preview) = focused_preview else {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        "No Editor document selected — choose a USD document in the Twin Browser.",
                    );
                });
                return;
            };
            let Some(state) = views.sessions.get_mut(&preview) else {
                ui.centered_and_justified(|ui| {
                    ui.label("The selected USD preview is still being projected.");
                });
                return;
            };
            if !state.built {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        "No Editor document selected — choose a USD document in the Twin Browser.",
                    );
                });
                return;
            }

            if state.schema_roots.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.vertical_centered(|ui| {
                        ui.heading("Generated connections");
                        ui.label("No authored USD connection schema is present.");
                        ui.label(
                            "The executable topology is generated from the composed USD network and is available in the standard Modelica diagram.",
                        );
                        let entries = ctx
                            .resource::<lunco_modelica::state::GeneratedModelicaSources>()
                            .map(|sources| sources.entries.clone())
                            .unwrap_or_default();
                        if entries.is_empty() {
                            ui.label("No generated network is available for this scene yet.");
                        } else {
                            for entry in entries {
                                let label = entry
                                    .uri
                                    .strip_prefix("generated://")
                                    .unwrap_or(entry.uri.as_str());
                                let label = label.strip_suffix(".mo").unwrap_or(label);
                                if let Some(error) = entry.projection_error {
                                    ui.colored_label(
                                        ui.visuals().error_fg_color,
                                        format!("{label}: {error}"),
                                    );
                                } else if entry.document.is_unassigned() {
                                    ui.label(format!("{label}: document is still compiling"));
                                } else if ui.button(format!("Open {label} diagram")).clicked() {
                                    ctx.trigger(FocusDocumentByName {
                                        pattern: label.to_string(),
                                    });
                                }
                            }
                        }
                    });
                });
                return;
            }

            let mut requested_root = state.active_schema_root.clone().unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label("System:");
                egui::ComboBox::from_id_salt("usd_schema_root")
                    .selected_text(
                        requested_root
                            .rsplit('/')
                            .next()
                            .filter(|leaf| !leaf.is_empty())
                            .unwrap_or("Select a schema"),
                    )
                    .show_ui(ui, |ui| {
                        for root in &state.schema_roots {
                            let label = root
                                .rsplit('/')
                                .next()
                                .filter(|leaf| !leaf.is_empty())
                                .unwrap_or(root);
                            ui.selectable_value(&mut requested_root, root.clone(), label)
                                .on_hover_text(root);
                        }
                    });
            });
            if !requested_root.is_empty()
                && state.active_schema_root.as_deref() != Some(requested_root.as_str())
            {
                let (nodes, wires) = project_schema(
                    state.source_nodes.clone(),
                    state.source_wires.clone(),
                    &requested_root,
                );
                state.canvas.scene = build_scene(nodes.clone(), wires.clone());
                state.canvas.selection.clear();
                state.topo_hash = topology_hash(&nodes, &wires);
                state.active_schema_root = Some(requested_root);
                state.needs_fit = state.canvas.scene.bounds().is_some();
            }

            if state.active_schema_root.is_none() {
                ui.centered_and_justified(|ui| {
                    ui.label("Select an authored schema to inspect its connections.");
                });
                return;
            }

            if state.canvas.scene.node_count() == 0 {
                ui.centered_and_justified(|ui| {
                    ui.label("The selected schema root has no authored schema nodes or connections.");
                });
                return;
            }

            // Snapshot origins + sinks BEFORE `ui` mutates the scene, so deleted
            // nodes/edges can still be resolved for their write-back op.
            let node_origin: HashMap<NodeId, String> = state
                .canvas
                .scene
                .nodes()
                .filter_map(|(id, n)| n.origin.clone().map(|o| (*id, o)))
                .collect();
            let edge_sinks: HashMap<EdgeId, EdgeSink> = state
                .canvas
                .scene
                .edges()
                .filter_map(|(id, _)| edge_sink(&state.canvas.scene, *id).map(|s| (*id, s)))
                .collect();
            let (Some(doc), Some(edit_target)) = (state.doc, state.edit_target.clone()) else {
                ui.centered_and_justified(|ui| {
                    ui.label("The selected USD preview has no writable authoring target.");
                });
                return;
            };

            // Consume a pending frame-to-fit now that the real widget size is
            // known (the producer can only guess it).
            if state.needs_fit {
                if let Some(b) = state.canvas.scene.bounds() {
                    let size = ui.available_size();
                    let rect = lunco_canvas::Rect::from_min_max(
                        lunco_canvas::Pos::new(0.0, 0.0),
                        lunco_canvas::Pos::new(size.x.max(1.0), size.y.max(1.0)),
                    );
                    let (c, z) = state.canvas.viewport.fit_values(b, rect, 48.0);
                    state.canvas.viewport.snap_to(c, z);
                }
                state.needs_fit = false;
            }

            ui.horizontal(|ui| {
                ui.small("Signals flow toward the arrowhead");
                ui.separator();
                ui.colored_label(lunco_theme::active(ui.ctx()).tokens.port_input, "input");
                ui.colored_label(lunco_theme::active(ui.ctx()).tokens.port_output, "output");
                ui.small("Names come from the USD port contract");
            });
            let (_resp, events) = state.canvas.ui(ui);
            if events.is_empty() {
                return;
            }
            let ops = build_ops(
                &state.canvas.scene,
                &node_origin,
                &edge_sinks,
                &events,
                &edit_target,
            );
            if ops.is_empty() {
                return;
            }
            ctx.trigger(lunco_usd::commands::ApplyUsdOps {
                doc,
                parent_gen: Some(state.generation),
                label: "Edit USD connections".to_string(),
                ops,
            });
        });
    }
}
