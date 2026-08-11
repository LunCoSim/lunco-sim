//! USD **connection canvas** — a node-graph view of a scene's wiring.
//!
//! A second projector over the generic `lunco-canvas` substrate (the Modelica
//! diagram is the first). It reads the live composed USD stage and renders each
//! wiring-relevant prim as a node and each co-sim connection / physics joint as
//! an edge; dragging port-to-port authors a `SetConnection`, and Delete clears
//! a wire or removes a prim — all through the journaled `ApplyUsdOp` path.
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
//!         └── SceneEvent ───┘  → UsdOp (SetConnection / RemovePrim) → ApplyUsdOp
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
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot};

use lunco_doc::{DocumentId, DocumentOrigin};
use lunco_doc_bevy::DocumentRegistry;
use lunco_usd::commands::ApplyUsdOp;
use lunco_usd::document::UsdDocument;
use lunco_usd::document::{LayerId, UsdOp};
use lunco_usd::ui::viewport::UsdViewportState;
use lunco_usd_bevy::{CanonicalStages, UsdPrimPath, UsdStageAsset};

use projection::{
    build_scene, collect_graph, project_schema, schema_roots, PrimNode, UsdPrimNodeData,
    UsdWireData, Wire, WireKind, EDGE_KIND, NODE_KIND,
};

pub const USD_CANVAS_PANEL_ID: PanelId = PanelId("usd_connection_canvas");

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

/// Panel state: the canvas plus the bindings the producer resolves so the
/// write-back path knows which document to author into.
#[derive(Resource)]
pub struct UsdCanvasState {
    canvas: Canvas,
    /// Stage currently projected — used to detect a scene swap.
    stage_id: Option<AssetId<UsdStageAsset>>,
    /// Editable document backing `stage_id`, if resolvable. `None` for a
    /// raw-file scene that has no `DocumentRegistry<UsdDocument>` entry — edits are
    /// suppressed (they'd be silently dropped on reboot).
    doc: Option<DocumentId>,
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

impl Default for UsdCanvasState {
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

/// View-model producer (WP-8): reads the live composed stage and rebuilds the
/// canvas scene when the topology changes. Runs on the main thread because
/// `StageView` is `!Send`.
pub fn produce_usd_canvas(
    q: Query<&UsdPrimPath>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<CanonicalStages>,
    asset_server: Res<AssetServer>,
    usd_registry: Option<Res<DocumentRegistry<UsdDocument>>>,
    viewport_state: Option<Res<UsdViewportState>>,
    mut state: ResMut<UsdCanvasState>,
) {
    // Pick the scene stage = the stage id with the most prim entities.
    let mut counts: HashMap<AssetId<UsdStageAsset>, (usize, Handle<UsdStageAsset>)> =
        HashMap::new();
    for p in q.iter() {
        let entry = counts
            .entry(p.stage_handle.id())
            .or_insert((0, p.stage_handle.clone()));
        entry.0 += 1;
    }
    let Some((stage_id, handle)) = counts
        .into_iter()
        .max_by_key(|(_, (c, _))| *c)
        .map(|(id, (_, h))| (id, h))
    else {
        return; // no USD prims yet
    };

    // Ensure the canonical stage is built (mirrors rewire_usd_connections).
    if canonical.get(stage_id).is_none() {
        if let Some(recipe) = stages.get(&handle).and_then(|a| a.recipe.clone()) {
            canonical.get_or_build(stage_id, &recipe);
        }
    }
    let Some(cs) = canonical.get(stage_id) else {
        return;
    };
    // Enumerate prims from the ECS entities on this stage (parity with the
    // co-sim wiring derivation) rather than a live-stage traversal, which can
    // miss composed children.
    let prim_paths: Vec<String> = q
        .iter()
        .filter(|p| p.stage_handle.id() == stage_id)
        .map(|p| p.path.clone())
        .collect();
    let view = cs.view();
    let (source_nodes, source_wires) = collect_graph(&view, &prim_paths);
    let roots = schema_roots(&source_nodes);
    let active_root = state
        .active_schema_root
        .as_ref()
        .filter(|root| roots.contains(root))
        .cloned()
        .or_else(|| roots.first().cloned());
    // The Connections perspective is an explicit authored boundary projection. The
    // collector above remains complete so the same USD topology stays
    // available to simulation, diagnostics, and future full-graph views.
    let (nodes, wires) = active_root
        .as_deref()
        .map(|root| project_schema(source_nodes.clone(), source_wires.clone(), root))
        .unwrap_or_default();
    let hash = topology_hash(&nodes, &wires);

    if state.built && state.stage_id == Some(stage_id) && state.topo_hash == hash {
        // The selected projection is unchanged, but another authored schema
        // root may have changed on this same stage revision. Retain the fresh
        // complete topology so switching systems never reveals stale data.
        state.source_nodes = source_nodes;
        state.source_wires = source_wires;
        state.schema_roots = roots;
        state.active_schema_root = active_root;
        return;
    }

    let scene = build_scene(nodes, wires);
    let bounds = scene.bounds();
    bevy::log::debug!(
        "[usd-canvas] rebuilt: {} prim entities -> {} nodes, {} edges",
        prim_paths.len(),
        scene.node_count(),
        scene.edge_count()
    );

    state.canvas.scene = scene;
    // Stale NodeId/EdgeId references would point at unrelated prims in the fresh
    // scene (ids restart at 0), so drop the selection on a structural rebuild.
    state.canvas.selection.clear();
    state.source_nodes = source_nodes;
    state.source_wires = source_wires;
    state.schema_roots = roots;
    state.active_schema_root = active_root;
    state.topo_hash = hash;
    state.stage_id = Some(stage_id);
    state.built = true;
    state.doc = resolve_doc(
        &handle,
        &asset_server,
        usd_registry.as_deref(),
        viewport_state.as_deref(),
    );

    // Request a frame-to-fit after every topology rebuild.  Composed scenes
    // can acquire late Modelica participants while their programs compile;
    // fitting only the first snapshot leaves the subsequently-added graph
    // outside the viewport.  The actual fit runs in the panel's next render,
    // which alone knows the real widget size (`F` re-fits precisely anytime
    // thereafter).  Structural rebuilds are the one case where restoring the
    // complete authored graph is more important than preserving a transient
    // pan position.
    if bounds.is_some() {
        state.needs_fit = true;
    }
}

/// Resolve the editable document backing a scene's stage handle — the same
/// suffix-match-then-active-doc fallback the Inspector uses for its own edits.
fn resolve_doc(
    handle: &Handle<UsdStageAsset>,
    asset_server: &AssetServer,
    usd_registry: Option<&DocumentRegistry<UsdDocument>>,
    viewport_state: Option<&UsdViewportState>,
) -> Option<DocumentId> {
    let by_path = asset_server
        .get_path(handle.id())
        .zip(usd_registry)
        .and_then(|(asset_path, reg)| {
            let path_str = asset_path.path().to_string_lossy().to_string();
            reg.ids().find(|id| {
                reg.host(*id)
                    .map(|h| match h.document().origin() {
                        DocumentOrigin::File { path, .. } => {
                            path.to_string_lossy().ends_with(&path_str)
                        }
                        _ => false,
                    })
                    .unwrap_or(false)
            })
        });
    by_path.or_else(|| viewport_state.and_then(|v| v.active_doc()))
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
fn connect_op(scene: &Scene, from: &PortRef, to: &PortRef) -> Option<UsdOp> {
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
        edit_target: LayerId::root(),
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
) -> Vec<UsdOp> {
    let mut ops = Vec::new();
    for ev in events {
        match ev {
            SceneEvent::EdgeCreated { from, to, .. } => {
                if let Some(op) = connect_op(scene, from, to) {
                    ops.push(op);
                }
            }
            SceneEvent::EdgeDeleted { id } => {
                if let Some(sink) = edge_sinks.get(id) {
                    ops.push(UsdOp::SetConnection {
                        edit_target: LayerId::root(),
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
                            edit_target: LayerId::root(),
                            path: sink.prim.clone(),
                            name: format!("inputs:{}", sink.connector),
                            type_name: "float".to_string(),
                            sources: Vec::new(),
                        });
                    }
                }
                if let Some(path) = node_origin.get(id) {
                    ops.push(UsdOp::RemovePrim {
                        edit_target: LayerId::root(),
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
        "🔗 Connections".into()
    }
    fn menu_group(&self) -> lunco_workbench::PanelMenuGroup {
        lunco_workbench::PanelMenuGroup::Scene
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Center
    }
    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        ctx.resource_scope::<UsdCanvasState, ()>(|ctx, state| {
            if !state.built {
                ui.centered_and_justified(|ui| {
                    ui.label("No wired scene loaded — open a USD scene to see its connections.");
                });
                return;
            }

            if state.schema_roots.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.vertical_centered(|ui| {
                        ui.heading("No authored connection schema");
                        ui.label(
                            "This scene has no prim marked lunco:ui:schemaRoot. Connections does not substitute the complete runtime topology.",
                        );
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
            if state.active_schema_root.as_deref() != Some(requested_root.as_str()) {
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
            let doc = state.doc;

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
            let ops = build_ops(&state.canvas.scene, &node_origin, &edge_sinks, &events);
            if ops.is_empty() {
                return;
            }
            match doc {
                Some(doc) => {
                    for op in ops {
                        ctx.trigger(ApplyUsdOp { doc, op });
                    }
                }
                None => {
                    // Raw-file scene: authoring would be dropped on reboot. The
                    // canvas already reflects the edit locally; the producer will
                    // reconcile it away on its next pass.
                    bevy::log::warn!(
                        "[usd-canvas] {} edit(s) ignored — scene is not document-backed",
                        ops.len()
                    );
                    ctx.trigger(lunco_core::TelemetryEvent {
                        name: "usd-connection-edit-failed".to_string(),
                        source: 0,
                        severity: lunco_core::Severity::Error,
                        data: lunco_core::TelemetryValue::String(
                            "Connections cannot be edited because this scene has no authoring document"
                                .to_string(),
                        ),
                        timestamp: 0.0,
                    });
                }
            }
        });
    }
}
