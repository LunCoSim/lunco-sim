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
//! scene or document; later typed scene-change batches
//! refresh only affected prim subtrees. A layout is rebuilt only when the
//! projected graph changes, so unrelated edits, pan / zoom / drag, and
//! selection preserve the current canvas. Rhai chooses automatic placement on
//! bounded workers; named view documents retain independent manual placements. The Diagram mode inspects
//! full USD topology with cached hierarchy navigation; Authored schema mode
//! edits explicitly marked boundaries through the document command owner.

mod drop_assets;
mod layout;
mod navigation;
mod projection;
mod toolbar;
mod view_files;
mod visuals;
pub use layout::{LayoutJobs, update_layouts};
pub use view_files::{init_view_commands, poll_view_files, view_files_pending};

use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_canvas::{Canvas, EdgeId, NodeId, PortRef, Scene, SceneEvent, VisualRegistry};
use lunco_workbench_core::{
    Panel, PanelCtx, PanelId, PanelScrollPolicy, PanelSlot, WorkbenchSnapshot,
};

use lunco_doc::diagram_view::DiagramViewDocument;
use lunco_doc::{Document, DocumentHost, DocumentId};
use lunco_usd_bevy_scene::{UsdPrimPath, UsdSceneChangeBatch};
use lunco_usd_bevy_stage::{UsdRead, UsdStageAsset, canonical::CanonicalStages};
use lunco_usd_document::document::{LayerId, UsdOp};
use lunco_usd_viewport_core::{UsdPreviewId, UsdPreviewSession, UsdViewportState};

use projection::{
    EDGE_KIND, NODE_KIND, PrimNode, UsdPrimNodeData, UsdWireData, Wire, WireKind, build_scene,
    collect_graph, collect_prim, project_diagram, project_schema, replace_affected_projection,
    schema_roots, wire_owners_affected_by_paths,
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
            programs: String::new(),
            accent: None,
            is_body: false,
            is_boundary: false,
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

/// One scene mount or preview lease canvas plus its resolved bindings so the
/// write-back path knows which document and authored layer to use.
pub struct UsdCanvasSessionState {
    canvas: Canvas,
    canvas_rect: Option<lunco_canvas::Rect>,
    source_root: Option<Entity>,
    source_uri: Option<String>,
    entities: HashMap<String, Entity>,
    view_document: Option<DocumentHost<DiagramViewDocument>>,
    selected_view: String,
    new_view_name: String,
    view_file_path: String,
    saved_view_generation: u64,
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
    layout_revision: u64,
    layout_request: Option<lunco_hooks::HookValue>,
    navigation_history: Vec<String>,
    scope_search: String,
    /// Complete collected topology retained so changing the active authored
    /// schema root is a presentation operation, not a stage reload.
    source_nodes: Vec<PrimNode>,
    source_wires: Vec<Wire>,
    /// All composed prim paths, including prims without ECS projections.
    /// Structural deltas use this index to re-read only the affected subtree.
    source_prim_paths: BTreeSet<String>,
    /// Changes received while the preview projection is settling.
    pending_changes: CanvasStageChanges,
    schema_roots: Vec<String>,
    active_schema_root: Option<String>,
    /// Explicit full-topology inspection versus authored schema editing.
    diagram_mode: bool,
    diagram_root: String,
    include_descendants: bool,
    /// Cached hierarchy choices, derived only when source topology changes.
    diagram_roots: BTreeSet<String>,
    /// Projected links that cannot resolve against authored node interfaces.
    unresolved_links: Vec<String>,
    published_diagnostic: String,
    published_layout_revision: u64,
    published_error: Option<String>,
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
        canvas.viewport.config.zoom_min = 0.001;
        Self {
            canvas,
            canvas_rect: None,
            source_root: None,
            source_uri: None,
            entities: HashMap::new(),
            view_document: None,
            selected_view: "Overview".into(),
            new_view_name: String::new(),
            view_file_path: String::new(),
            saved_view_generation: 0,
            stage_id: None,
            doc: None,
            edit_target: None,
            generation: 0,
            canonical_generation: None,
            topo_hash: 0,
            built: false,
            needs_fit: false,
            layout_revision: 0,
            layout_request: None,
            navigation_history: Vec::new(),
            scope_search: String::new(),
            source_nodes: Vec::new(),
            source_wires: Vec::new(),
            source_prim_paths: BTreeSet::new(),
            pending_changes: CanvasStageChanges::default(),
            schema_roots: Vec::new(),
            active_schema_root: None,
            diagram_mode: true,
            diagram_root: "/".into(),
            include_descendants: false,
            diagram_roots: BTreeSet::new(),
            unresolved_links: Vec::new(),
            published_diagnostic: String::new(),
            published_layout_revision: 0,
            published_error: None,
            last_error: None,
        }
    }
}

impl UsdCanvasSessionState {
    fn clear(&mut self) {
        self.layout_revision = self.layout_revision.wrapping_add(1);
        self.layout_request = None;
        self.navigation_history.clear();
        self.scope_search.clear();
        self.source_root = None;
        self.source_uri = None;
        self.entities.clear();
        self.view_document = None;
        self.selected_view = "Overview".into();
        self.view_file_path.clear();
        self.new_view_name.clear();
        self.saved_view_generation = 0;
        self.canvas.scene = Scene::default();
        self.canvas_rect = None;
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
        self.diagram_mode = true;
        self.diagram_root = "/".into();
        self.include_descendants = false;
        self.diagram_roots.clear();
        self.unresolved_links.clear();
        self.published_diagnostic.clear();
        self.last_error = None;
    }

    /// Reproject cached facts after explicit navigation; never reread USD in paint.
    fn rebuild_view(&mut self) {
        let (nodes, wires) = self.project_view();
        self.topo_hash = topology_hash(&nodes, &wires);
        self.unresolved_links = projection::unresolved_links(&nodes, &wires);
        self.canvas.scene = build_scene(nodes, wires);
        layout::request(self);
        self.restore_placements();
        self.canvas.selection.clear();
        self.needs_fit = self.canvas.scene.bounds().is_some();
    }

    fn restore_placements(&mut self) {
        let Some(definition) = self
            .view_document
            .as_ref()
            .and_then(|host| host.document().data().views.get(&self.selected_view))
        else {
            return;
        };
        let placements = &definition.positions;
        let ids: Vec<_> = self.canvas.scene.nodes().map(|(id, _)| *id).collect();
        for id in ids {
            let Some(node) = self.canvas.scene.node_mut(id) else {
                continue;
            };
            if let Some(path) = projection::diagram_key(node) {
                if let Some(pos) = placements.get(path) {
                    if pos.x.abs() > f64::from(f32::MAX) || pos.y.abs() > f64::from(f32::MAX) {
                        self.last_error = Some(format!(
                            "View position for {path} exceeds the canvas rendering range"
                        ));
                        continue;
                    }
                    // Explicit validated f64 -> f32 canvas rendering boundary.
                    let canvas_position = lunco_canvas::Pos::new(pos.x as f32, pos.y as f32);
                    node.rect = lunco_canvas::Rect::from_min_size(
                        canvas_position,
                        node.rect.width(),
                        node.rect.height(),
                    );
                }
            }
        }
        projection::route_edges(&mut self.canvas.scene);
    }

    fn project_view(&self) -> (Vec<PrimNode>, Vec<Wire>) {
        if self.diagram_mode {
            project_diagram(
                &self.source_nodes,
                &self.source_wires,
                &self.diagram_root,
                self.include_descendants,
            )
        } else {
            self.active_schema_root
                .as_deref()
                .map(|root| project_schema(&self.source_nodes, &self.source_wires, root))
                .unwrap_or_default()
        }
    }
}

/// Active scene and session-keyed connection canvases. Interaction state (pan, zoom,
/// graph selection, and chosen schema root) belongs to its preview lease.
#[derive(Resource)]
pub struct UsdCanvasState {
    scene: UsdCanvasSessionState,
    show_scene: bool,
    sessions: HashMap<UsdPreviewId, UsdCanvasSessionState>,
}

impl Default for UsdCanvasState {
    fn default() -> Self {
        Self {
            scene: Default::default(),
            show_scene: true,
            sessions: Default::default(),
        }
    }
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
        n.connectors.hash(&mut h);
        n.port_types.hash(&mut h);

        n.port_sources.hash(&mut h);
        n.programs.hash(&mut h);
        n.variants.hash(&mut h);
        n.usd_origin.hash(&mut h);
        n.boundary.hash(&mut h);
        n.referenced_ports.hash(&mut h);
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

/// Build subsystem choices from graph participants and their USD ancestry.
/// Every composed prim can expose its own interface schema or child topology.
fn diagram_roots(nodes: &[PrimNode]) -> BTreeSet<String> {
    let mut roots = BTreeSet::from(["/".to_string()]);
    for node in nodes {
        roots.insert(node.path.clone());
        let mut path = node.path.as_str();
        while let Some((parent, _)) = path.rsplit_once('/') {
            if parent.is_empty() {
                break;
            }
            roots.insert(parent.to_string());
            path = parent;
        }
    }
    roots
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

/// View-model producer (WP-8): reads active scene and open preview composed stages and
/// rebuilds its canvas scene when the topology changes. Runs on the main thread
/// because `StageView` is `!Send`.
pub fn produce_usd_canvas(
    q: Query<(Entity, &UsdPrimPath)>,
    path_changes: Query<(Entity, &UsdPrimPath), Changed<UsdPrimPath>>,
    q_parents: Query<&ChildOf>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<CanonicalStages>,
    viewport_state: Option<Res<UsdViewportState>>,
    mounts: Res<lunco_core::SceneMountState>,
    workbench: Option<Res<WorkbenchSnapshot>>,
    mut views: ResMut<UsdCanvasState>,
    mut scene_change_reader: MessageReader<UsdSceneChangeBatch>,
) {
    let viewport = viewport_state.as_deref();
    let open: HashSet<_> = viewport
        .into_iter()
        .flat_map(|v| v.sessions())
        .map(|s| s.id())
        .collect();
    views.sessions.retain(|preview, _| open.contains(preview));
    let live = mounts
        .active_root()
        .and_then(|root| q.get(root).ok().map(|(_, path)| (root, path)));
    if live.is_none() {
        views.scene.clear();
    }
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
    if let Some((root, path)) = live {
        produce_usd_canvas_session(
            None,
            path.stage_handle.clone(),
            root,
            None,
            0,
            true,
            &q,
            &path_changes,
            &q_parents,
            &stages,
            &mut canonical,
            &mut views.scene,
            changes_by_stage.get(&path.stage_handle.id()),
        );
    }
    for session in viewport.into_iter().flat_map(|v| v.sessions()) {
        let state = views.sessions.entry(session.id()).or_default();
        produce_usd_canvas_session(
            Some(session.doc()),
            session.stage_handle().clone(),
            session.scene_root(),
            Some(session.edit_target().clone()),
            session.projected_generation(),
            session.projection_ready(),
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
    doc: Option<DocumentId>,
    handle: Handle<UsdStageAsset>,
    preview_root: Entity,
    edit_target: Option<LayerId>,
    generation: u64,
    ready: bool,
    q: &Query<(Entity, &UsdPrimPath)>,
    path_changes: &Query<(Entity, &UsdPrimPath), Changed<UsdPrimPath>>,
    q_parents: &Query<&ChildOf>,
    stages: &Assets<UsdStageAsset>,
    canonical: &mut CanonicalStages,
    state: &mut UsdCanvasSessionState,
    changes: Option<&CanvasStageChanges>,
) {
    let stage_id = handle.id();

    // A lease replacement invalidates the complete interaction model before a
    // new stage becomes available; a loading document cannot show or edit the
    // previous document's graph.
    let identity_changed = state.doc != doc
        || state.stage_id != Some(stage_id)
        || state.source_root != Some(preview_root);
    if identity_changed {
        state.clear();
    }
    let mut accumulated_changes = std::mem::take(&mut state.pending_changes);
    if let Some(changes) = changes {
        accumulated_changes.merge(changes);
    }
    if !ready {
        state.pending_changes = accumulated_changes;
        return;
    }
    // The mount publishes its root entity before USD projection assigns the
    // root path. Admit the view only after that authoritative identity exists.
    let Ok((_, root_prim)) = q.get(preview_root) else {
        state.pending_changes = accumulated_changes;
        return;
    };
    if root_prim.path.is_empty() {
        state.pending_changes = accumulated_changes;
        return;
    }
    state.source_uri = stages
        .get(&handle)
        .and_then(|asset| asset.recipe.as_ref())
        .map(|recipe| recipe.root_id.clone());
    if state.view_document.is_none() {
        if let Some(uri) = state.source_uri.as_ref() {
            state.diagram_root = root_prim.path.clone();
            let mut host = DocumentHost::new(DiagramViewDocument::new(uri.clone()));
            let definition = lunco_doc::diagram_view::DiagramView {
                scope: state.diagram_root.clone(),
                include_descendants: false,
                positions: Default::default(),
            };
            if let Err(error) = host.apply(lunco_doc::Mutation::local(
                lunco_doc::diagram_view::DiagramViewOp::SetView {
                    name: "Overview".into(),
                    view: Some(definition),
                },
            )) {
                state.last_error = Some(error.to_string());
            }
            state.saved_view_generation = host.document().generation();
            state.view_document = Some(host);
        }
    }
    state.edit_target = edit_target;
    state.source_root = Some(preview_root);
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
            && (state.generation != generation
                || state.canonical_generation != Some(canonical_generation)));
    if !full_rebuild && !has_path_changes {
        return;
    }

    let view = cs.view();
    let prim_paths_for_log;
    if full_rebuild {
        state.entities = q
            .iter()
            .filter(|(entity, p)| p.stage_handle.id() == stage_id && is_preview_entity(*entity))
            .map(|(entity, p)| (p.path.clone(), entity))
            .collect();
        let prim_paths: Vec<String> = view
            .prim_paths()
            .into_iter()
            .map(|path| path.to_string())
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
                state.entities.remove(&path);
                affected_paths.insert(path);
            }
        }
        let mut pending = changes.resynced_roots.clone();
        while let Some(path) = pending.pop() {
            let Ok(prim) = openusd::sdf::Path::new(&path) else {
                state.last_error = Some(format!("Invalid USD change path: {path}"));
                bevy::log::warn!("[usd-canvas] invalid USD change path: {path}");
                return;
            };
            pending.extend(
                view.children(&prim)
                    .into_iter()
                    .map(|child| child.to_string()),
            );
            affected_paths.insert(path);
        }
        affected_paths.extend(changes.info_paths.iter().cloned());

        for (entity, path) in path_changes.iter() {
            if path.stage_handle.id() == stage_id
                && is_preview_entity(entity)
                && changes
                    .resynced_roots
                    .iter()
                    .any(|root| path_is_within(&path.path, root))
            {
                state.entities.insert(path.path.clone(), entity);
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

    projection::resolve_referenced_interfaces(&view, &mut state.source_nodes, &state.source_wires);
    let roots = schema_roots(&state.source_nodes);
    state.diagram_roots = diagram_roots(&state.source_nodes);
    if !state.diagram_roots.contains(&state.diagram_root) {
        state.last_error = Some(format!(
            "USD scope {} is unavailable; showing the source with automatic layout",
            state.diagram_root
        ));
        state.diagram_root = "/".into();
    }
    let active_root = state
        .active_schema_root
        .as_ref()
        .filter(|root| roots.contains(root))
        .cloned();
    state.active_schema_root = active_root.clone();
    let (nodes, wires) = state.project_view();
    let hash = topology_hash(&nodes, &wires);

    if state.built && state.stage_id == Some(stage_id) && state.topo_hash == hash {
        state.schema_roots = roots;
        state.active_schema_root = active_root;
        state.generation = generation;
        state.canonical_generation = Some(canonical_generation);
        return;
    }

    state.unresolved_links = projection::unresolved_links(&nodes, &wires);
    let scene = build_scene(nodes, wires);
    let bounds = scene.bounds();
    bevy::log::debug!(
        "[usd-canvas] root {:?} rebuilt: {} prim entities -> {} nodes, {} edges",
        preview_root,
        prim_paths_for_log,
        scene.node_count(),
        scene.edge_count()
    );
    state.canvas.scene = scene;
    layout::request(state);
    state.restore_placements();
    state.canvas.selection.clear();
    state.schema_roots = roots;
    state.active_schema_root = active_root;
    state.topo_hash = hash;
    state.stage_id = Some(stage_id);
    state.built = true;
    state.doc = doc;
    state.generation = generation;
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
    mounts: Res<lunco_core::SceneMountState>,
    revision: Res<lunco_usd_bevy_scene::UsdStageRevision>,
    workbench: Option<Res<WorkbenchSnapshot>>,
) -> bool {
    let viewport_changed = viewport.is_some_and(|state| state.is_changed());
    let visible = workbench
        .as_deref()
        .is_some_and(|snapshot| snapshot.is_panel_visible(USD_CANVAS_PANEL_ID));
    viewport_changed
        || mounts.is_changed()
        || (visible
            && (revision.is_changed() || workbench.is_some_and(|snapshot| snapshot.is_changed())))
}

// ─── Write-back: SceneEvent → UsdOp ─────────────────────────────────────────

/// Authored endpoint lists captured before a canvas event removes its edge.
struct EdgeSink {
    endpoints: Vec<(String, String, String, Vec<String>, String)>,
}

fn edge_sink(scene: &Scene, id: EdgeId) -> Option<EdgeSink> {
    let edge = scene.edge(id)?;
    let kind = edge.data.downcast_ref::<UsdWireData>()?.kind;
    if kind == WireKind::Joint {
        return None;
    }
    let mut endpoints = Vec::new();
    for (sink, source) in [(&edge.to, &edge.from), (&edge.from, &edge.to)] {
        let sink_node = scene.node(sink.node)?;
        let source_node = scene.node(source.node)?;
        let data = sink_node.data.downcast_ref::<UsdPrimNodeData>()?;
        let source_path = format!(
            "{}.{}",
            source_node.origin.as_deref()?,
            source.port.as_str()
        );
        let sources = data
            .port_sources
            .get(sink.port.as_str())
            .cloned()
            .unwrap_or_default();
        if sources.contains(&source_path) {
            endpoints.push((
                sink_node.origin.clone()?,
                sink.port.as_str().into(),
                data.port_types.get(sink.port.as_str())?.clone(),
                sources,
                source_path,
            ));
        }
    }
    Some(EdgeSink { endpoints })
}

/// Resolve causal direction or selected-system interface forwarding and author
/// the exact sink property. Presentation identities never enter USD edits.
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
    let boundary = |pr: &PortRef| {
        scene
            .node(pr.node)
            .and_then(|node| node.data.downcast_ref::<UsdPrimNodeData>())
            .and_then(|data| data.boundary)
    };
    let (source, sink) = match (from_kind, to_kind) {
        ("output", "input") => (from, to),
        ("input", "output") => (to, from),
        ("acausal", "acausal") => (from, to),
        ("input", "input") if boundary(from) == Some(projection::BoundaryRole::Inputs) => {
            (from, to)
        }
        ("input", "input") if boundary(to) == Some(projection::BoundaryRole::Inputs) => (to, from),
        ("output", "output") if boundary(to) == Some(projection::BoundaryRole::Outputs) => {
            (from, to)
        }
        ("output", "output") if boundary(from) == Some(projection::BoundaryRole::Outputs) => {
            (to, from)
        }
        _ => {
            return Err(format!(
                "cannot connect `{from_kind}` to `{to_kind}`; use an output/input pair or two acausal connectors"
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
    let property_type = |endpoint: &PortRef| {
        scene
            .node(endpoint.node)
            .and_then(|node| node.data.downcast_ref::<UsdPrimNodeData>())
            .and_then(|node| node.port_types.get(endpoint.port.as_str()))
            .ok_or_else(|| {
                format!(
                    "USD port {} has no declared property type",
                    endpoint.port.as_str()
                )
            })
    };
    let source_type = property_type(source)?;
    let sink_type = property_type(sink)?;
    if source_type != sink_type {
        return Err(format!("cannot connect USD {source_type} to {sink_type}"));
    }
    let mut sources = scene
        .node(sink.node)
        .and_then(|node| node.data.downcast_ref::<UsdPrimNodeData>())
        .and_then(|data| data.port_sources.get(sink_conn))
        .cloned()
        .unwrap_or_default();
    let source = format!("{source_prim}.{source_conn}");
    if !sources.contains(&source) {
        sources.push(source);
    }
    Ok(UsdOp::SetConnection {
        edit_target: edit_target.clone(),
        path: sink_prim,
        name: sink_conn.to_string(),
        type_name: sink_type.clone(),
        sources,
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
    let mut removals: std::collections::BTreeMap<(String, String), (String, Vec<String>)> =
        Default::default();
    let mut remove = |sink: &EdgeSink| {
        for (prim, connector, type_name, sources, source) in &sink.endpoints {
            let entry = removals
                .entry((prim.clone(), connector.clone()))
                .or_insert_with(|| (type_name.clone(), sources.clone()));
            entry.1.retain(|path| path != source);
        }
    };
    for ev in events {
        match ev {
            SceneEvent::EdgeCreated { from, to, .. } => {
                ops.push(connect_op(scene, from, to, edit_target)?);
            }
            SceneEvent::EdgeDeleted { id } => {
                if let Some(sink) = edge_sinks.get(id) {
                    remove(sink);
                } else {
                    return Err("this link has no editable USD property connection; edit physics joints in the joint editor".into());
                }
            }
            SceneEvent::NodeDeleted { id, orphaned_edges } => {
                // Clear any dataflow wire that fed this prim, then remove it.
                for eid in orphaned_edges {
                    if let Some(sink) = edge_sinks.get(eid) {
                        remove(sink);
                    }
                }
                if let Some(path) = node_origin.get(id) {
                    ops.push(UsdOp::RemovePrim {
                        edit_target: edit_target.clone(),
                        path: path.clone(),
                    });
                } else {
                    return Err("System interface terminals cannot be deleted; edit the USD properties instead".into());
                }
            }
            _ => {}
        }
    }
    let mut clear_ops = Vec::new();
    for ((path, name), (type_name, sources)) in removals {
        clear_ops.push(UsdOp::SetConnection {
            edit_target: edit_target.clone(),
            path,
            name,
            type_name,
            sources,
        });
    }
    clear_ops.extend(ops);
    Ok(clear_ops)
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
            let show_scene = views.show_scene;
            let preview = focused_preview;
            let state = if show_scene {
                &mut views.scene
            } else {
                let Some(preview) = preview else { toolbar::source_picker(ui, &mut views.show_scene); ui.label("Choose a USD document in the Twin Browser."); return; };
                if !projection_ready { ui.label("The selected USD preview is settling."); return; }
                let Some(state) = views.sessions.get_mut(&preview) else { ui.label("The document is being projected."); return; };
                state
            };
            if !state.built { toolbar::source_picker(ui, &mut views.show_scene); ui.label("The loaded USD scene is being projected."); return; }
            if let Some(show_scene) = toolbar::render(ui, ctx, state, show_scene) {
                views.show_scene = show_scene;
                return;
            }
            if !state.diagram_mode && state.active_schema_root.is_none() {
                ui.label("Select an authored schema to inspect its connections.");
                return;
            }

            if state.canvas.scene.node_count() == 0 {
                ui.centered_and_justified(|ui| {
                    ui.label("This system has no visible models. Use the breadcrumbs to return to its parent.");
                });
                return;
            }

            // Snapshot origins + sinks BEFORE `ui` mutates the scene, so deleted
            // nodes/edges can still be resolved for their write-back op.
            state.canvas.read_only = show_scene;
            state.canvas.movable_layout = true;
            let node_origin: HashMap<NodeId, String> = if show_scene { HashMap::new() } else { state
                .canvas
                .scene
                .nodes()
                .filter_map(|(id, n)| n.data.downcast_ref::<UsdPrimNodeData>().filter(|data| data.boundary.is_none()).and_then(|_| n.origin.clone()).map(|o| (*id, o)))
                .collect() };
            let edge_sinks: HashMap<EdgeId, EdgeSink> = if show_scene { HashMap::new() } else { state
                .canvas
                .scene
                .edges()
                .filter_map(|(id, _)| edge_sink(&state.canvas.scene, *id).map(|s| (*id, s)))
                .collect() };
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

            let (response, events) = state.canvas.ui(ui);
            state.canvas_rect = Some(lunco_canvas::Rect::from_min_max(lunco_canvas::Pos::new(response.rect.min.x, response.rect.min.y), lunco_canvas::Pos::new(response.rect.max.x, response.rect.max.y)));
            drop_assets::drop_ui(ui, &response, state, ctx);
            if events.is_empty() {
                return;
            }

            for event in &events {
                match event {
                    SceneEvent::NodeMoved { id, new_min, .. } => {
                        if let Some(path) = state.canvas.scene.node(*id).and_then(projection::diagram_key).map(str::to_string) {
                            if let Some(host) = state.view_document.as_ref() {
                                ctx.trigger(view_files::MoveConnectionViewNode { view_id: host.document().id().raw(), view: state.selected_view.clone(), path, x: f64::from(new_min.x), y: f64::from(new_min.y) });
                            }
                        }
                    }
                    SceneEvent::NodeDoubleClicked { id } if state.diagram_mode => {
                        if let (Some(key), Some(host)) = (state.canvas.scene.node(*id).and_then(projection::diagram_key), state.view_document.as_ref()) {
                            ctx.trigger(navigation::OpenConnectionNode { view_id: host.document().id().raw(), key: key.into(), program_path: None });
                        }
                    }
                    SceneEvent::SelectionChanged(selection) => {
                        if let Some(path) = selection.nodes().iter().next()
                            .and_then(|id| state.canvas.scene.node(*id))
                            .and_then(|node| node.origin.clone()) {
                            if show_scene {
                                if let Some(target) = state.entities.get(&path) {
                                    ctx.trigger(lunco_scene_selection::SelectEntityTarget { target: *target, intent: lunco_scene_selection::SelectionIntent::Replace });
                                }
                            } else if let Some(preview) = preview {
                                ctx.trigger(crate::selection::SelectUsdPrim { preview, path, extend: false, toggle: false });
                            }
                        }
                    }
                    _ => {}
                }
            }
            if show_scene {
                return;
            }
            let (Some(doc), Some(edit_target)) = (state.doc, state.edit_target.clone()) else {
                state.rebuild_view();
                state.last_error = Some("No writable USD document target".into());
                return;
            };
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
                    state.rebuild_view();
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
