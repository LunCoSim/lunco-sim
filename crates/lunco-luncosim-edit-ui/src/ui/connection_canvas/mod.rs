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
//! The producer runs on the **main thread** (the stage is `!Send`) only while
//! its workbench panel is visible. Initial admission reads the complete
//! preview; later typed scene-change batches
//! refresh only affected prim subtrees. A layout is rebuilt only when the
//! projected graph changes, so unrelated edits, pan / zoom / drag, and
//! selection preserve the current canvas. Node *positions* are session-only
//! for v1 — a structural graph edit re-lays-out; persisting a
//! `lunco:canvasPos` is a follow-up.

mod projection;
mod visuals;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_canvas::{Canvas, EdgeId, NodeId, PortRef, Scene, SceneEvent, VisualRegistry};
use lunco_workbench_core::{
    Panel, PanelCtx, PanelId, PanelScrollPolicy, PanelSlot, WorkbenchSnapshot,
};

use lunco_doc::DocumentId;
use lunco_modelica_ui_core::FocusDocumentByName;
use lunco_usd_bevy_scene::{UsdPrimPath, UsdSceneChangeBatch};
use lunco_usd_bevy_stage::{UsdStageAsset, canonical::CanonicalStages};
use lunco_usd_document::document::{LayerId, UsdOp};
use lunco_usd_viewport_core::{UsdPreviewId, UsdPreviewSession, UsdViewportState};

use projection::{
    EDGE_KIND, NODE_KIND, PrimNode, UsdPrimNodeData, UsdWireData, Wire, WireKind, build_scene,
    collect_graph, collect_prim, project_schema, replace_affected_projection, schema_roots,
    wire_owners_affected_by_paths,
};

pub use lunco_usd_ui::USD_CONNECTION_CANVAS_PANEL_ID as USD_CANVAS_PANEL_ID;

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
    /// Canonical stage generation represented by the cached path and graph data.
    canonical_generation: Option<u64>,
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
    /// All entity-backed prim paths, including paths irrelevant to the graph.
    /// Structural deltas use this index to re-read only the affected subtree.
    source_prim_paths: BTreeSet<String>,
    /// Changes received while the preview projection is settling.
    pending_changes: CanvasStageChanges,
    schema_roots: Vec<String>,
    active_schema_root: Option<String>,
    /// Last rejected graph edit. Keep it next to the graph so an invalid drag
    /// cannot disappear as a no-op between frames.
    last_error: Option<String>,
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
            canonical_generation: None,
            topo_hash: 0,
            built: false,
            needs_fit: false,
            source_nodes: Vec::new(),
            source_wires: Vec::new(),
            source_prim_paths: BTreeSet::new(),
            pending_changes: CanvasStageChanges::default(),
            schema_roots: Vec::new(),
            active_schema_root: None,
            last_error: None,
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
        self.canonical_generation = None;
        self.topo_hash = 0;
        self.built = false;
        self.needs_fit = false;
        self.source_nodes.clear();
        self.source_wires.clear();
        self.source_prim_paths.clear();
        self.pending_changes = CanvasStageChanges::default();
        self.schema_roots.clear();
        self.active_schema_root = None;
        self.last_error = None;
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
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for n in nodes {
        n.path.hash(&mut h);
        n.type_name.hash(&mut h);
        n.is_body.hash(&mut h);
        n.schema_root.hash(&mut h);
        n.schema_node.hash(&mut h);
        n.display_name.hash(&mut h);
        n.schema_column.hash(&mut h);
        n.schema_row.hash(&mut h);
        n.inputs.hash(&mut h);
        n.outputs.hash(&mut h);
    }
    for w in wires {
        w.kind.hash(&mut h);
        w.source_path.hash(&mut h);
        w.source_conn.hash(&mut h);
        w.target_path.hash(&mut h);
        w.target_conn.hash(&mut h);
    }
    h.finish()
}

#[derive(Clone, Default)]
struct CanvasStageChanges {
    stage_generation: Option<u64>,
    resynced_roots: Vec<String>,
    info_paths: Vec<String>,
}

impl CanvasStageChanges {
    fn merge(&mut self, changes: &Self) {
        if changes.stage_generation.is_some() {
            self.stage_generation = changes.stage_generation;
        }
        self.resynced_roots
            .extend(changes.resynced_roots.iter().cloned());
        self.info_paths.extend(changes.info_paths.iter().cloned());
        self.resynced_roots.sort_unstable();
        self.resynced_roots.dedup();
        self.info_paths.sort_unstable();
        self.info_paths.dedup();
    }

    fn has_paths(&self) -> bool {
        !self.resynced_roots.is_empty() || !self.info_paths.is_empty()
    }
}

fn path_is_within(path: &str, root: &str) -> bool {
    path == root
        || (root == "/" && path.starts_with('/'))
        || path
            .strip_prefix(root)
            .is_some_and(|remainder| remainder.starts_with('/'))
}

fn indexed_subtree(paths: &BTreeSet<String>, root: &str) -> Vec<String> {
    // USD child paths form a contiguous lexical range after their exact root.
    paths
        .range(root.to_string()..)
        .take_while(|path| path_is_within(path, root))
        .cloned()
        .collect()
}

fn sort_graph(nodes: &mut [PrimNode], wires: &mut [Wire]) {
    nodes.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    wires.sort_unstable_by(|a, b| {
        (
            a.owner_path.as_str(),
            a.source_path.as_str(),
            a.source_conn.as_str(),
            a.target_path.as_str(),
            a.target_conn.as_str(),
            matches!(a.kind, WireKind::Joint),
        )
            .cmp(&(
                b.owner_path.as_str(),
                b.source_path.as_str(),
                b.source_conn.as_str(),
                b.target_path.as_str(),
                b.target_conn.as_str(),
                matches!(b.kind, WireKind::Joint),
            ))
    });
}

/// View-model producer (WP-8): reads each open preview's composed stage and
/// rebuilds its canvas scene when the topology changes. Runs on the main thread
/// because `StageView` is `!Send`.
pub fn produce_usd_canvas(
    q: Query<(Entity, &UsdPrimPath)>,
    path_changes: Query<(Entity, &UsdPrimPath), Changed<UsdPrimPath>>,
    q_parents: Query<&ChildOf>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<CanonicalStages>,
    viewport_state: Option<Res<UsdViewportState>>,
    workbench: Option<Res<WorkbenchSnapshot>>,
    mut views: ResMut<UsdCanvasState>,
    mut scene_change_reader: MessageReader<UsdSceneChangeBatch>,
) {
    let Some(viewport) = viewport_state.as_deref() else {
        views.sessions.clear();
        return;
    };
    let open: std::collections::HashSet<_> = viewport.sessions().map(|s| s.id()).collect();
    views.sessions.retain(|preview, _| open.contains(preview));
    if !workbench
        .as_deref()
        .is_some_and(|snapshot| snapshot.is_panel_visible(USD_CANVAS_PANEL_ID))
    {
        return;
    }
    let mut changes_by_stage: HashMap<AssetId<UsdStageAsset>, CanvasStageChanges> = HashMap::new();
    for change in scene_change_reader.read() {
        let changes = changes_by_stage.entry(change.stage_id).or_default();
        changes.stage_generation = Some(change.stage_generation);
        changes
            .resynced_roots
            .extend(change.resynced_prim_paths.iter().cloned());
        changes
            .info_paths
            .extend(change.info_prim_paths.iter().cloned());
    }
    for session in viewport.sessions() {
        let state = views.sessions.entry(session.id()).or_default();
        produce_usd_canvas_session(
            session,
            &q,
            &path_changes,
            &q_parents,
            &stages,
            &mut canonical,
            state,
            changes_by_stage.get(&session.stage_handle().id()),
        );
    }
}

fn produce_usd_canvas_session(
    session: &UsdPreviewSession,
    q: &Query<(Entity, &UsdPrimPath)>,
    path_changes: &Query<(Entity, &UsdPrimPath), Changed<UsdPrimPath>>,
    q_parents: &Query<&ChildOf>,
    stages: &Assets<UsdStageAsset>,
    canonical: &mut CanonicalStages,
    state: &mut UsdCanvasSessionState,
    changes: Option<&CanvasStageChanges>,
) {
    let doc = session.doc();
    let handle = session.stage_handle().clone();
    let preview_root = session.scene_root();
    let stage_id = handle.id();

    // A lease replacement invalidates the complete interaction model before a
    // new stage becomes available; a loading document cannot show or edit the
    // previous document's graph.
    let identity_changed = state.doc != Some(doc) || state.stage_id != Some(stage_id);
    if identity_changed {
        state.clear();
    }
    let mut accumulated_changes = std::mem::take(&mut state.pending_changes);
    if let Some(changes) = changes {
        accumulated_changes.merge(changes);
    }
    if !session.projection_ready() {
        state.pending_changes = accumulated_changes;
        return;
    }
    state.edit_target = Some(session.edit_target().clone());
    let is_preview_entity =
        |entity: Entity| lunco_usd_bevy_scene::is_preview_entity(entity, preview_root, q_parents);
    if canonical.get(stage_id).is_none() {
        if let Some(recipe) = stages.get(&handle).and_then(|a| a.recipe.clone()) {
            canonical.get_or_build(stage_id, &recipe);
        }
    }
    let Some(cs) = canonical.get(stage_id) else {
        return;
    };
    let canonical_generation = cs.generation();
    let has_path_changes = accumulated_changes.has_paths();
    let changes = accumulated_changes
        .stage_generation
        .is_some()
        .then_some(&accumulated_changes);
    let delta_is_current = changes.is_some_and(|changes| {
        changes.stage_generation == Some(canonical_generation)
            && state.canonical_generation
                == changes
                    .stage_generation
                    .map(|generation| generation.wrapping_sub(1))
    });
    let full_rebuild = identity_changed
        || !state.built
        || (has_path_changes && !delta_is_current)
        || (!has_path_changes
            && (state.generation != session.projected_generation()
                || state.canonical_generation != Some(canonical_generation)));
    if !full_rebuild && !has_path_changes {
        return;
    }

    let view = cs.view();
    let prim_paths_for_log;
    if full_rebuild {
        let prim_paths: Vec<String> = q
            .iter()
            .filter(|(entity, p)| p.stage_handle.id() == stage_id && is_preview_entity(*entity))
            .map(|(_, p)| p.path.clone())
            .collect();
        let (mut source_nodes, mut source_wires) = collect_graph(&view, &prim_paths);
        sort_graph(&mut source_nodes, &mut source_wires);
        state.source_nodes = source_nodes;
        state.source_wires = source_wires;
        state.source_prim_paths = prim_paths.iter().cloned().collect();
        prim_paths_for_log = prim_paths.len();
    } else {
        let changes = changes.expect("incremental projection has a path change");
        let mut affected_paths: HashSet<String> = HashSet::new();
        for root in &changes.resynced_roots {
            let subtree = indexed_subtree(&state.source_prim_paths, root);
            for path in subtree {
                state.source_prim_paths.remove(&path);
                affected_paths.insert(path);
            }
        }
        affected_paths.extend(changes.resynced_roots.iter().cloned());
        affected_paths.extend(changes.info_paths.iter().cloned());

        for (entity, path) in path_changes.iter() {
            if path.stage_handle.id() == stage_id
                && is_preview_entity(entity)
                && changes
                    .resynced_roots
                    .iter()
                    .any(|root| path_is_within(&path.path, root))
            {
                affected_paths.insert(path.path.clone());
            }
        }

        let affected_wire_owners = wire_owners_affected_by_paths(
            &state.source_wires,
            &changes.resynced_roots,
            &changes.info_paths,
        );
        let mut paths_to_read: HashSet<String> = affected_paths.clone();
        paths_to_read.extend(affected_wire_owners.iter().cloned());
        let mut paths_to_read: Vec<String> = paths_to_read.into_iter().collect();
        paths_to_read.sort_unstable();

        let mut replacement_nodes = Vec::with_capacity(affected_paths.len());
        let mut replacement_wires = Vec::new();
        for path in &paths_to_read {
            if let Some(mut projection) = collect_prim(&view, path) {
                if affected_paths.contains(path) {
                    state.source_prim_paths.insert(path.clone());
                    if let Some(node) = projection.node {
                        replacement_nodes.push(node);
                    }
                }
                replacement_wires.append(&mut projection.wires);
            }
        }
        replace_affected_projection(
            &mut state.source_nodes,
            &mut state.source_wires,
            &changes.resynced_roots,
            &changes.info_paths,
            &affected_wire_owners,
            replacement_nodes,
            replacement_wires,
        );
        sort_graph(&mut state.source_nodes, &mut state.source_wires);
        prim_paths_for_log = state.source_prim_paths.len();
    }

    let roots = schema_roots(&state.source_nodes);
    let active_root = state
        .active_schema_root
        .as_ref()
        .filter(|root| roots.contains(root))
        .cloned();
    let (nodes, wires) = active_root
        .as_deref()
        .map(|root| project_schema(&state.source_nodes, &state.source_wires, root))
        .unwrap_or_default();
    let hash = topology_hash(&nodes, &wires);

    if state.built && state.stage_id == Some(stage_id) && state.topo_hash == hash {
        state.schema_roots = roots;
        state.active_schema_root = active_root;
        state.generation = session.projected_generation();
        state.canonical_generation = Some(canonical_generation);
        return;
    }

    let scene = build_scene(nodes, wires);
    let bounds = scene.bounds();
    bevy::log::debug!(
        "[usd-canvas] preview {} rebuilt: {} prim entities -> {} nodes, {} edges",
        session.id().0,
        prim_paths_for_log,
        scene.node_count(),
        scene.edge_count()
    );
    state.canvas.scene = scene;
    state.canvas.selection.clear();
    state.schema_roots = roots;
    state.active_schema_root = active_root;
    state.topo_hash = hash;
    state.stage_id = Some(stage_id);
    state.built = true;
    state.doc = Some(doc);
    state.generation = session.projected_generation();
    state.canonical_generation = Some(canonical_generation);
    if bounds.is_some() {
        state.needs_fit = true;
    }
}

/// Wake the connection graph for visible-panel changes and authored stage
/// updates. Preview lifecycle changes still wake the producer while hidden so
/// closed preview state is retired without reading the composed stage.
pub fn editor_canvas_changed(
    viewport: Option<Res<UsdViewportState>>,
    revision: Res<lunco_usd_bevy_scene::UsdStageRevision>,
    workbench: Option<Res<WorkbenchSnapshot>>,
) -> bool {
    let viewport_changed = viewport.is_some_and(|state| state.is_changed());
    let visible = workbench
        .as_deref()
        .is_some_and(|snapshot| snapshot.is_panel_visible(USD_CANVAS_PANEL_ID));
    viewport_changed
        || (visible
            && (revision.is_changed() || workbench.is_some_and(|snapshot| snapshot.is_changed())))
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
fn connect_op(
    scene: &Scene,
    from: &PortRef,
    to: &PortRef,
    edit_target: &LayerId,
) -> Result<UsdOp, String> {
    let kind = |pr: &PortRef| -> Result<&str, String> {
        scene
            .node(pr.node)
            .ok_or_else(|| format!("connection endpoint node {:?} no longer exists", pr.node))?
            .ports
            .iter()
            .find(|p| p.id == pr.port)
            .map(|p| p.kind.as_str())
            .ok_or_else(|| format!("connection endpoint port `{:?}` no longer exists", pr.port))
    };
    let from_kind = kind(from)?;
    let to_kind = kind(to)?;
    let (source, sink) = match (from_kind, to_kind) {
        ("output", "input") => (from, to),
        ("input", "output") => (to, from),
        _ => {
            return Err(format!(
                "cannot connect `{from_kind}` to `{to_kind}`; dataflow connections require an output and an input"
            ));
        }
    };
    let source_prim = scene
        .node(source.node)
        .and_then(|node| node.origin.clone())
        .ok_or_else(|| format!("source node {:?} has no USD prim origin", source.node))?;
    let sink_prim = scene
        .node(sink.node)
        .and_then(|node| node.origin.clone())
        .ok_or_else(|| format!("sink node {:?} has no USD prim origin", sink.node))?;
    let sink_conn = sink.port.as_str();
    let source_conn = source.port.as_str();
    Ok(UsdOp::SetConnection {
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
) -> Result<Vec<UsdOp>, String> {
    let mut ops = Vec::new();
    for ev in events {
        match ev {
            SceneEvent::EdgeCreated { from, to, .. } => {
                ops.push(connect_op(scene, from, to, edit_target)?);
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
    Ok(ops)
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
    fn menu_group(&self) -> lunco_workbench_core::PanelMenuGroup {
        lunco_workbench_core::PanelMenuGroup::Editor
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Center
    }
    fn scroll_policy(&self) -> PanelScrollPolicy {
        PanelScrollPolicy::SelfManaged
    }
    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let viewport = ctx.resource::<UsdViewportState>();
        let focused_preview = viewport.and_then(UsdViewportState::focused_preview_id);
        let projection_ready = focused_preview
            .and_then(|preview| viewport.and_then(|state| state.session(preview)))
            .is_some_and(UsdPreviewSession::projection_ready);
        ctx.resource_scope::<UsdCanvasState, ()>(|ctx, views| {
            let Some(preview) = focused_preview else {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        "No Editor document selected — choose a USD document in the Twin Browser.",
                    );
                });
                return;
            };
            if !projection_ready {
                ui.centered_and_justified(|ui| {
                    ui.label("The selected USD preview is settling; connection editing is paused.");
                });
                return;
            }
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
                            .resource::<lunco_modelica_runtime::generated_source::GeneratedModelicaSources>()
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
                    &state.source_nodes,
                    &state.source_wires,
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
            if let Some(error) = state.last_error.as_deref() {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("Connection edit rejected: {error}"),
                );
            }
            let (_resp, events) = state.canvas.ui(ui);
            if events.is_empty() {
                return;
            }
            let ops = match build_ops(
                &state.canvas.scene,
                &node_origin,
                &edge_sinks,
                &events,
                &edit_target,
            ) {
                Ok(ops) => {
                    state.last_error = None;
                    ops
                }
                Err(error) => {
                    state.last_error = Some(error);
                    return;
                }
            };
            if ops.is_empty() {
                return;
            }
            ctx.trigger(lunco_usd_core::commands::ApplyUsdOps {
                doc_id: doc,
                parent_gen: Some(state.generation),
                label: "Edit USD connections".to_string(),
                ops,
            });
        });
    }
}
