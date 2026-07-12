//! Modelica diagram, rendered via `lunco-canvas`.
//!
//! Sole diagram path. The previous egui-snarl-backed `diagram.rs`
//! has been removed — `lunco-canvas` covers every feature we use.
//!
//! # Pipeline
//!
//! ```text
//!   ModelicaDocument (AST)                        (lunco-doc)
//!           │
//!           ▼
//!   VisualDiagram  (existing intermediate)        (lunco-modelica)
//!           │  project_scene()
//!           ▼
//!   lunco_canvas::Scene   →  Canvas   →  egui
//!           ▲                  │
//!           └──── SceneEvent ──┘      → (future) DocumentOp back to ModelicaDocument
//! ```
//!
//! # What's in B2
//!
//! - Read-side projector: `VisualDiagram → Scene` (one-shot on bind,
//!   rebuilt on doc generation change).
//! - Rectangle + label visuals; straight-line edges.
//! - Drag-to-move nodes → mutates the local scene (feedback only —
//!   doc ops from drag land in B3).
//! - Pan / zoom / select / rubber-band / Delete / F-to-fit — all via
//!   the default `Canvas` wiring, nothing to wire here.
//!
//! Icon rendering (SVG via `usvg`), animated wires, widget-in-node
//! plots, and doc-op emission all land later as new visual impls /
//! in the projector's write-back path — no canvas changes required.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_canvas::{Canvas, NavBarOverlay, VisualRegistry};
use lunco_workbench::PanelId;

pub const CANVAS_DIAGRAM_PANEL_ID: PanelId = PanelId("modelica_canvas_diagram");

// ─── Visuals ────────────────────────────────────────────────────────

mod theme;
mod paint;
pub mod loads;
mod port;
mod edge;
mod node;
mod projection;
mod palette;
mod overlays;
mod menus;
mod decorations;
mod pulse;
mod ops;
mod panel;
pub use theme::CanvasThemeSnapshot;
pub use panel::CanvasDiagramPanel;
pub(crate) use panel::invalidate_port_icon_cache;
// `__register_on_auto_arrange_diagram` is the registrar `#[on_command]` generates
// next to the handler; `register_commands!` in `ui::commands` names the observer by
// path, so the generated helper has to travel with it through this re-export.
pub use ops::{
    active_class_for_doc, active_class_for_doc_ctx, apply_ops_public, on_auto_arrange_diagram,
    __register_on_auto_arrange_diagram,
};
// Op-application core moved to the egui-free `crate::doc_ops` module.
pub use crate::doc_ops::{apply_one_op_as, drain_pending_structural_ops, PendingStructuralOps};
// API-feedback queue data moved to the egui-free `crate::canvas_feedback`.
pub use pulse::{
    EdgePulseHandle, PulseEntry, PulseHandle, drive_pending_api_connections,
    drive_pending_api_focus,
};
use pulse::{EdgePulseLayer, PulseGlowLayer};
pub use palette::{DiagramProjectionLimits, PaletteSettings};
pub use loads::{DrillInBinding, DuplicateBinding, drill_into_class, drive_drill_in_loads, drive_duplicate_loads};
pub use edge::ConnectionEdgeData;
pub use node::IconNodeData;
pub use projection::ProjectionTask;

use node::IconNodeVisual;
use paint::wire_color_for;
use edge::OrthogonalEdgeVisual;

















fn build_registry() -> VisualRegistry {
    let mut reg = VisualRegistry::new();
    // Generic in-canvas viz node kinds (plots today, dashboards /
    // cameras tomorrow). Lives in lunco-viz so it's reusable from any
    // domain plugin that wants embedded scopes — Modelica is just the
    // first integrator.
    lunco_viz::kinds::canvas_plot_node::register(&mut reg);
    crate::ui::text_node::register(&mut reg);
    reg.register_node_kind("modelica.icon", |data: &lunco_canvas::NodeData| {
        // Downcast to the typed payload the projector boxed (see
        // `IconNodeData`). Empty payload → render with defaults; the
        // visual handles a missing icon by showing the type label.
        let Some(d) = data.downcast_ref::<IconNodeData>() else {
            return IconNodeVisual::default();
        };
        let type_label = d
            .qualified_type
            .rsplit('.')
            .next()
            .unwrap_or(&d.qualified_type)
            .to_string();
        IconNodeVisual {
            type_label: type_label.clone(),
            class_name: type_label,
            icon_only: d.icon_only,
            expandable_connector: d.expandable_connector,
            icon_graphics: d.icon_graphics.clone(),
            parameters: d.parameters.clone(),
            rotation_deg: d.rotation_deg,
            mirror_x: d.mirror_x,
            mirror_y: d.mirror_y,
            instance_name: d.instance_name.clone(),
            port_connector_paths: d.port_connector_paths.clone(),
            port_connector_icons: d.port_connector_icons.clone(),
            is_conditional: d.is_conditional,
            parent_qualified_type: d.qualified_type.clone(),
        }
    });
    reg.register_edge_kind("modelica.connection", |data: &lunco_canvas::NodeData| {
        let Some(d) = data.downcast_ref::<ConnectionEdgeData>() else {
            return OrthogonalEdgeVisual::default();
        };
        let leaf = d
            .connector_type
            .rsplit('.')
            .next()
            .unwrap_or(&d.connector_type)
            .to_string();
        // PortKind from rumoca's typed classifier covers most cases,
        // but short-form connectors (`connector RealInput = input Real`)
        // currently land as `Acausal` because rumoca attaches the
        // `input`/`output` keyword to the alias-target, not to the
        // classifier's `class.causality` slot. Pattern-match the
        // canonical Modelica signal connector names so the wire still
        // renders as causal (thicker stroke + arrowhead at input).
        let causal_by_name = leaf.ends_with("Input") || leaf.ends_with("Output");
        let is_causal = matches!(
            d.kind,
            crate::visual_diagram::PortKind::Input | crate::visual_diagram::PortKind::Output,
        ) || causal_by_name;
        // Materialise the per-frame HashMap lookup keys here, once per
        // projection — avoids two `format!()` allocations per edge per
        // frame in `OrthogonalEdgeVisual::draw`.
        let flow_lookup_keys = d.flow_vars.first().map(|fv| (
            format!("{}.{}", d.source_path, fv.name),
            format!("{}.{}", d.target_path, fv.name),
        ));
        OrthogonalEdgeVisual {
            color: d
                .icon_color
                .unwrap_or_else(|| wire_color_for(&d.connector_type)),
            from_dir: d.from_dir,
            to_dir: d.to_dir,
            is_causal,
            source_path: d.source_path.clone(),
            target_path: d.target_path.clone(),
            flow_vars: d.flow_vars.clone(),
            connector_leaf: leaf,
            flow_lookup_keys,
            smooth_bezier: d.smooth_bezier,
            thickness_scale: d.thickness_scale,
        }
    });
    reg
}

// ─── Projection: VisualDiagram → lunco_canvas::Scene ────────────────

/// Modelica diagram coordinates are `(-100..100)` both axes with +Y
/// up. Width is a fixed 20×20 world-unit box — the typical
/// Modelica icon extent (`{{-10,-10},{10,10}}`). Dymola/OMEdit
/// render components at this size by default. Reading the actual
/// per-component `Icon` annotation extent is a follow-up.
pub(super) const ICON_W: f32 = 20.0;
/// Coordinate-system types + the two conversion functions between
/// them. Named wrappers around plain `(f32, f32)` so every place
/// the sign flip happens is explicit and typed — previously we had
/// ad-hoc `-y` negations scattered across the projector, the op
/// emitters, and the context-menu handler, and a missing negation
/// or a double-negation produced the hard-to-diagnose "position is
/// off" class of bugs.
///
/// Conventions:
///
/// - [`crate::ui::panels::canvas_diagram::ModelicaPos`] — Modelica `.mo` source convention. +Y up.
///   Ranges typically `-100..100` per axis. This is the authored
///   coordinate that lands in `annotation(Placement(...))`.
///
/// - [`lunco_canvas::Pos`] — canvas world coordinate. +Y DOWN
///   (screen convention). This is what the canvas scene / viewport
///   consume and what hit-testing / rendering operates on.
///
/// The two differ only in the sign of Y. Keeping them as separate
/// types makes mis-conversion a type error instead of a silent off-
/// by-flip.
pub mod coords {
    use lunco_canvas::Pos as CanvasPos;

    /// Modelica-convention 2D point (+Y up).
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct ModelicaPos {
        pub x: f32,
        pub y: f32,
    }

    impl ModelicaPos {
        pub const fn new(x: f32, y: f32) -> Self {
            Self { x, y }
        }
    }

    /// Canvas world (+Y down) → Modelica (+Y up).
    #[inline]
    pub fn canvas_to_modelica(c: CanvasPos) -> ModelicaPos {
        ModelicaPos {
            x: c.x,
            y: -c.y,
        }
    }

    /// Modelica (+Y up) → canvas world (+Y down).
    #[inline]
    pub fn modelica_to_canvas(m: ModelicaPos) -> CanvasPos {
        CanvasPos::new(m.x, -m.y)
    }

    /// Canvas rect-min → Modelica centre. Used when committing a
    /// drag: the user's drag target lands as the icon's top-left in
    /// canvas coordinates, but Modelica placements are centre-
    /// anchored, so we shift by half the icon extent.
    #[inline]
    pub fn canvas_min_to_modelica_center(
        min: CanvasPos,
        icon_w: f32,
        icon_h: f32,
    ) -> ModelicaPos {
        canvas_to_modelica(CanvasPos::new(
            min.x + icon_w * 0.5,
            min.y + icon_h * 0.5,
        ))
    }
}



// ─── Panel state + Bevy resource ───────────────────────────────────

/// Per-document canvas state. Each open model tab owns one of
/// these, keyed by [`lunco_doc::DocumentId`] on [`CanvasDiagramState`]. Holds
/// the transform + selection + in-flight projection task for that
/// specific document so switching tabs doesn't leak viewport,
/// selection, or a stale projection into a neighbour.
/// Shared handle to the target class's `Diagram(graphics={...})`
/// annotation — painted as canvas background by
/// `DiagramDecorationLayer`. Projector updates it each time the
/// drilled-in class changes.
pub type BackgroundDiagramHandle = std::sync::Arc<
    std::sync::RwLock<
        Option<(
            crate::annotations::CoordinateSystem,
            Vec<crate::annotations::GraphicItem>,
            Vec<crate::annotations::LunCoPlotNode>,
        )>,
    >,
>;

pub struct CanvasDocState {
    pub canvas: Canvas,
    pub last_seen_gen: u64,
    /// Generation that the *canvas scene* already reflects, ahead of
    /// or equal to the AST projection. Bumped by `apply_ops` when a
    /// canvas-originated edit has already been applied locally
    /// (drag → SetPlacement leaves the scene moved; menu Add → a
    /// synthesised node is inserted into the scene). The project gate
    /// then skips reprojection while `canvas_acked_gen >= gen`,
    /// which is what keeps Add and Move feeling instant: no waiting
    /// for the off-thread parse to complete before the visual
    /// settles. The next *foreign* edit (typed source change) bumps
    /// `gen` past `canvas_acked_gen` and the regular projection path
    /// re-engages.
    pub canvas_acked_gen: u64,
    /// Hash of the *projection-relevant* slice of source for the
    /// scene currently on screen — collapses whitespace, drops
    /// comments. Cheap-skip: when a doc generation bumps but this
    /// hash is unchanged (a comment edit, a parameter-default tweak,
    /// added blank lines), we mark the gen as seen without spawning
    /// a projection task. Catches the bulk of typing latency.
    ///
    /// TODO(partial-reproject): replace this binary skip with an
    /// AST-diff path. Compare prev vs new `ClassDef.components` /
    /// `equations` / annotations, emit a sequence of
    /// `DiagramOp { AddNode | RemoveNode | MoveNode | AddEdge |
    /// RemoveEdge | RelabelNode }`, and apply each to `canvas.scene`
    /// in place. Falls back to full reproject on extends/within/
    /// multi-class changes. Needs (1) Scene mutation API surface
    /// (move/relabel/add-without-rebuild), (2) `diff_class(old,
    /// new) -> Vec<DiagramOp>` helper, (3) origin-name as stable
    /// node identity (already true). 30 % of edits hit the partial
    /// path — see <follow-up issue> when ready.
    pub last_seen_source_hash: u64,
    /// Set by the [`crate::ui::commands::FitCanvas`] observer; the
    /// canvas render system consumes it next frame and runs Fit
    /// against the *actual* widget rect (rather than the hardcoded
    /// 800×600 the observer would have to use). Cleared after the
    /// fit lands.
    pub pending_fit: bool,
    /// Snapshot of the drill-in target that produced the *currently
    /// rendered* scene. The render trigger compares this against the
    /// live `DrilledInClassNames[doc_id]`; a difference re-projects.
    /// Without this, clicking a class in the Twin Browser updated the
    /// drill-in resource but the canvas kept showing the previous
    /// target's cached scene — the visible "click did nothing" bug.
    pub last_seen_target: Option<String>,
    pub context_menu: Option<PendingContextMenu>,
    pub projection_task: Option<ProjectionTask>,
    /// Background decoration — the target class's own
    /// `Diagram(graphics={...})` annotation. Painted by the
    /// decoration layer registered on `canvas`. Shared via `Arc` so
    /// the projection code can update the layer's data without
    /// reaching into `canvas.layers`.
    pub background_diagram: BackgroundDiagramHandle,
    /// Per-doc pulse-glow registry. The `drive_pending_api_focus`
    /// system writes new entries when an API-driven AddComponent's
    /// node lands in the projected scene; the `PulseGlowLayer` reads
    /// this every draw and paints a Figma-style outer-glow ring with
    /// alpha decaying over `PULSE_DURATION`. Shared by `Arc` so both
    /// sides see the same vec without walking the layer list.
    pub pulse_handle: PulseHandle,
    /// Per-doc edge-pulse registry — same shape as `pulse_handle`
    /// but for newly-added connections. Drives the wire-flash
    /// rendered by `EdgePulseLayer`.
    pub edge_pulse_handle: EdgePulseHandle,
    /// Hot-exit zoom/pan restore: a saved [`lunco_canvas::Viewport`]
    /// seeded when this tab's entry is created for a *restored* document
    /// (see [`CanvasDiagramState::stash_pending_view`]). The initial
    /// projection consumes it (`take`) and `snap_to`s the saved camera
    /// instead of running fit-to-content, so a reopened diagram looks
    /// exactly as it did at exit. `None` for normally-opened docs.
    pub pending_view: Option<lunco_canvas::Viewport>,
    /// One-shot re-projection request. Set on every open tab when the MSL
    /// standard library finishes loading (web fetches + decodes it
    /// asynchronously a few seconds after boot). A diagram projected *before*
    /// MSL was resident draws its standard-library components — `FixedHeatFlow`,
    /// `Gain`, … — as blank placeholder boxes, because neither icon source
    /// (pre-baked index / engine session) had the class yet. This flag forces
    /// one in-place re-projection so those icons resolve, **without** resetting
    /// the camera (a `last_seen_gen = 0` reset would re-fit). Consumed +
    /// cleared by `spawn_projection_task`.
    pub force_reproject: bool,
}

impl Default for CanvasDocState {
    fn default() -> Self {
        let mut canvas = Canvas::new(build_registry());
        canvas.layers.retain(|layer| layer.name() != "selection");
        canvas.overlays.push(Box::new(NavBarOverlay::default()));
        // Diagram decoration layer sits right after the grid so it
        // paints behind nodes and edges. The decoration data is
        // shared via `Arc<RwLock>` with `CanvasDocState` so the
        // projector can swap in a new class's graphics without
        // walking the layer list.
        let background_diagram: BackgroundDiagramHandle =
            std::sync::Arc::new(std::sync::RwLock::new(None));
        let decoration_idx = canvas
            .layers
            .iter()
            .position(|l| l.name() != "grid")
            .unwrap_or(canvas.layers.len());
        canvas.layers.insert(
            decoration_idx,
            Box::new(decorations::DiagramDecorationLayer {
                data: background_diagram.clone(),
            }),
        );
        // Pulse-glow layer goes at the END of the layer list so the
        // ring paints ON TOP of nodes/edges/selection — matches
        // Figma's outer-glow which is visible regardless of underlying
        // chrome. See `docs/architecture/20-domain-modelica.md` § 9c.4.
        let pulse_handle: PulseHandle =
            std::sync::Arc::new(std::sync::RwLock::new(Vec::new()));
        canvas.layers.push(Box::new(PulseGlowLayer {
            data: pulse_handle.clone(),
        }));
        let edge_pulse_handle: EdgePulseHandle =
            std::sync::Arc::new(std::sync::RwLock::new(Vec::new()));
        canvas.layers.push(Box::new(EdgePulseLayer {
            data: edge_pulse_handle.clone(),
        }));
        Self {
            canvas,
            last_seen_gen: 0,
            canvas_acked_gen: 0,
            last_seen_source_hash: 0,
            pending_fit: false,
            last_seen_target: None,
            context_menu: None,
            projection_task: None,
            background_diagram,
            pulse_handle,
            edge_pulse_handle,
            pending_view: None,
            force_reproject: false,
        }
    }
}



/// Per-panel state carried across frames. Stored as a Bevy resource
/// so the panel's `render` can pull it out via `world.resource_mut`.
///
/// State is sharded per-document — each open model tab has its own
/// [`crate::ui::panels::canvas_diagram::CanvasDocState`] entry so viewport/selection/projection/context
/// menu never bleed between tabs. `fallback` is used only when no
/// document is bound (startup, every tab closed).
/// Per-tab key for [`CanvasDiagramState`]. Each tab owns its own
/// `CanvasDocState` (viewport, selection, scene, projection task)
/// so two tabs viewing the same `(doc, drilled_class)` can pan,
/// zoom, and select independently.
pub type CanvasKey = crate::model_tabs_types::TabId;

#[derive(Resource, Default)]
pub struct CanvasDiagramState {
    per_tab: std::collections::HashMap<CanvasKey, CanvasDocState>,
    /// Doc-id sidecar for fast `drop_doc(doc)` and `iter_doc_ids`.
    /// Updated on every `get_mut_for_tab` insert.
    tab_doc: std::collections::HashMap<CanvasKey, lunco_doc::DocumentId>,
    fallback: CanvasDocState,
    /// Parse→project handoff slot. When a driver (duplicate, drill-in,
    /// file-load) resolves its parse task, it moves its `StatusBus`
    /// `BusyHandle` here so the bus stays busy for `Document(doc_id)`
    /// across the frame boundary where the parse handle would otherwise
    /// drop before the projection task spawns. The next
    /// `spawn_projection_task` for the doc minted its own handle and
    /// then calls [`complete_projection_handoff`], releasing this one.
    /// Without this slot the bus blinks empty between parse-complete
    /// and project-spawn, and the canvas overlay flickers off then on.
    pending_projection_handoff:
        std::collections::HashMap<lunco_doc::DocumentId, lunco_workbench::status_bus::BusyHandle>,
    /// Hot-exit camera restore, keyed by document. Populated by
    /// [`stash_pending_view`](Self::stash_pending_view) when a document
    /// is restored from the per-Twin workspace-state (its tab doesn't
    /// exist yet at restore time). When the tab's `CanvasDocState` is
    /// first created in [`get_mut_for_tab`](Self::get_mut_for_tab) the
    /// saved viewport is moved onto it; the initial projection then
    /// `snap_to`s it instead of fitting.
    pending_view_restore:
        std::collections::HashMap<lunco_doc::DocumentId, lunco_canvas::Viewport>,
}

impl CanvasDiagramState {
    /// Stash the parse-phase `BusyHandle` so the `StatusBus` keeps an
    /// entry under `Document(doc_id)` until the next projection spawn
    /// for the doc registers its own. Replaces any previous stashed
    /// handle for the same doc (its `Drop` then clears the older bus
    /// entry safely — see `re_begin_evicts_prior_handle_silently`).
    pub fn stash_projection_handoff(
        &mut self,
        doc: lunco_doc::DocumentId,
        handle: lunco_workbench::status_bus::BusyHandle,
    ) {
        self.pending_projection_handoff.insert(doc, handle);
    }

    /// Mark every open tab (and the shared fallback) for a one-shot
    /// re-projection. Called when the MSL standard library becomes resident so
    /// diagrams projected before its icons were available re-resolve them
    /// (otherwise standard-library components stay as blank boxes until the
    /// source is next edited). Re-projects in place — no camera reset.
    pub fn request_reproject_all(&mut self) {
        for st in self.per_tab.values_mut() {
            st.force_reproject = true;
        }
        self.fallback.force_reproject = true;
    }

    /// Drop the handoff handle for `doc`, if any. Called from
    /// `spawn_projection_task` after the new projection entry is on
    /// the bus, so total bus busy-count for the doc never reaches
    /// zero during the parse→project transition.
    pub fn complete_projection_handoff(&mut self, doc: lunco_doc::DocumentId) {
        self.pending_projection_handoff.remove(&doc);
    }

    /// Legacy single-doc lookup. Resolves to the first tab viewing
    /// `doc`, or the shared fallback when `doc` is `None` /
    /// no tab has been opened yet. Non-render callers (event
    /// observers, ops layer) still use this; the canvas render path
    /// keys explicitly by `get_for_tab`.
    pub fn get(&self, doc: Option<lunco_doc::DocumentId>) -> &CanvasDocState {
        match doc.and_then(|d| self.first_tab_for(d)) {
            Some(tab_id) => self.per_tab.get(&tab_id).unwrap_or(&self.fallback),
            None => &self.fallback,
        }
    }

    /// Legacy mutable lookup; routes to the first tab viewing
    /// `doc`. **Does not allocate** on a `None`/missing-doc path —
    /// returns the fallback. Callers that *need* an entry should
    /// pass an explicit `tab_id` via `get_mut_for_tab`.
    pub fn get_mut(
        &mut self,
        doc: Option<lunco_doc::DocumentId>,
    ) -> &mut CanvasDocState {
        match doc.and_then(|d| self.first_tab_for(d)) {
            Some(tab_id) => self
                .per_tab
                .get_mut(&tab_id)
                .unwrap_or(&mut self.fallback),
            None => &mut self.fallback,
        }
    }

    /// Read-only view scoped to a specific tab. Returns the
    /// fallback when `tab_id` has no entry yet — first-render path.
    pub fn get_for_tab(&self, tab_id: CanvasKey) -> &CanvasDocState {
        self.per_tab.get(&tab_id).unwrap_or(&self.fallback)
    }

    /// Mutable per-tab view, creating the entry on first access.
    /// `doc` is recorded in the sidecar so `drop_doc` /
    /// `iter_doc_ids` stay cheap.
    pub fn get_mut_for_tab(
        &mut self,
        tab_id: CanvasKey,
        doc: lunco_doc::DocumentId,
    ) -> &mut CanvasDocState {
        self.tab_doc.entry(tab_id).or_insert(doc);
        // Seed a hot-exit-restored camera onto the *first* tab opened for
        // a restored doc. `remove` returns `Some` only on that first
        // entry creation; later calls are a cheap empty-map miss.
        let restored_view = self.pending_view_restore.remove(&doc);
        let entry = self.per_tab.entry(tab_id).or_default();
        if let Some(v) = restored_view {
            entry.pending_view = Some(v);
        }
        entry
    }

    /// Stash a hot-exit-restored [`lunco_canvas::Viewport`] for `doc`.
    /// Applied to the doc's first tab when it's created (see
    /// [`get_mut_for_tab`](Self::get_mut_for_tab)). Called by the Modelica
    /// session codec on workspace-state restore.
    pub fn stash_pending_view(
        &mut self,
        doc: lunco_doc::DocumentId,
        view: lunco_canvas::Viewport,
    ) {
        self.pending_view_restore.insert(doc, view);
    }

    /// Iterate `(tab_id, doc_id)` for every tab known to the per-tab
    /// state map. Used by sibling-sync invalidation in `apply_ops` to
    /// find tabs viewing the just-edited doc.
    pub fn tab_doc_iter(&self) -> impl Iterator<Item = (CanvasKey, lunco_doc::DocumentId)> + '_ {
        self.tab_doc.iter().map(|(&tab, &doc)| (tab, doc))
    }

    /// Render-context lookup: route to the active tab when one is in
    /// scope (during a panel render), fall back to first-tab
    /// semantics otherwise (observers, off-render systems).
    ///
    /// This is the canonical lookup for code that runs inside the
    /// canvas render path or any UI handler called from it
    /// (right-click menus, palette drop, etc.). Outside render the
    /// `TabRenderContext.tab_id` is `None` and we fall back to the
    /// first-tab path — matches the legacy single-tab behaviour
    /// per-doc, which is what observer-time code wants.
    pub fn get_for_render(
        &self,
        render_tab_id: Option<CanvasKey>,
        doc: Option<lunco_doc::DocumentId>,
    ) -> &CanvasDocState {
        match render_tab_id {
            Some(t) => self.get_for_tab(t),
            None => self.get(doc),
        }
    }

    /// Mutable counterpart of `get_for_render`. When both
    /// `render_tab_id` and `doc` are populated, allocates a per-tab
    /// entry; otherwise routes through the legacy first-tab path.
    pub fn get_mut_for_render(
        &mut self,
        render_tab_id: Option<CanvasKey>,
        doc: Option<lunco_doc::DocumentId>,
    ) -> &mut CanvasDocState {
        match (render_tab_id, doc) {
            (Some(t), Some(d)) => self.get_mut_for_tab(t, d),
            _ => self.get_mut(doc),
        }
    }

    // `get_for(doc, drilled)` / `get_mut_for(doc, drilled)` migration
    // shims deleted. The `drilled` argument was always ignored
    // (drilled scopes are independent tabs since the Phase-1 tab
    // refactor) and no callers remained outside test code. Use
    // `get_for_render` / `get_mut_for_render` for tab-aware lookups
    // or `get` / `get_mut` for the legacy first-tab fallback.

    /// First TabId viewing `doc`. Determinism is best-effort
    /// (HashMap iteration); non-render callers don't care which
    /// tab they get because the underlying scene/source is
    /// identical across tabs of the same `(doc, drilled)`.
    fn first_tab_for(&self, doc: lunco_doc::DocumentId) -> Option<CanvasKey> {
        self.tab_doc
            .iter()
            .find_map(|(tab_id, d)| (*d == doc).then_some(*tab_id))
    }

    /// Drop *every* per-tab entry whose doc is `doc` when the
    /// document is removed from the registry. Also clears any
    /// pending parse→project handoff handle for the doc — without
    /// this, a doc closed mid-load (parse resolved, stash placed,
    /// no canvas render fired before close) would leak its
    /// `StatusBus` entry indefinitely.
    pub fn drop_doc(&mut self, doc: lunco_doc::DocumentId) {
        let to_drop: Vec<CanvasKey> = self
            .tab_doc
            .iter()
            .filter_map(|(t, d)| (*d == doc).then_some(*t))
            .collect();
        for t in to_drop {
            self.per_tab.remove(&t);
            self.tab_doc.remove(&t);
        }
        self.pending_projection_handoff.remove(&doc);
    }

    /// Drop a single tab's canvas state. Called when a tab is
    /// closed (different from closing a document).
    pub fn drop_tab(&mut self, tab_id: CanvasKey) {
        self.per_tab.remove(&tab_id);
        self.tab_doc.remove(&tab_id);
    }

    /// Iterate the *distinct* document ids that currently have
    /// canvas state across any open tab.
    pub fn iter_doc_ids(&self) -> impl Iterator<Item = lunco_doc::DocumentId> + '_ {
        let mut seen = std::collections::HashSet::new();
        self.tab_doc
            .values()
            .filter_map(move |d| seen.insert(*d).then_some(*d))
    }

    /// Read-only state for an explicit doc id, or `None` when no
    /// tab has been opened on it. Returns whichever tab matched
    /// first; for one-tab-per-doc consumers (the common case) this
    /// is the only one anyway.
    pub fn get_for_doc(
        &self,
        doc: lunco_doc::DocumentId,
    ) -> Option<&CanvasDocState> {
        self.first_tab_for(doc).and_then(|t| self.per_tab.get(&t))
    }

    /// Has any tab on this doc ever been projected?
    pub fn has_entry(&self, doc: lunco_doc::DocumentId) -> bool {
        self.first_tab_for(doc).is_some()
    }

    // `has_entry_for(doc, drilled)` migration shim deleted.
    // No callers; use `has_entry(doc)` directly.

    /// Has *this specific tab* ever been projected? Renders the
    /// canvas use this to force a first-paint projection on a
    /// freshly-mounted tab.
    pub fn has_entry_for_tab(&self, tab_id: CanvasKey) -> bool {
        self.per_tab.contains_key(&tab_id)
    }

    /// Mutable iterator over `(tab_id, doc, state)` triples. Used
    /// by the inactive-tab projection-cancel system to flip
    /// `projection_task.cancel` on every non-active tab.
    pub fn iter_mut(
        &mut self,
    ) -> impl Iterator<Item = (CanvasKey, lunco_doc::DocumentId, &mut CanvasDocState)> + '_
    {
        let tab_doc = &self.tab_doc;
        self.per_tab.iter_mut().map(move |(tab_id, state)| {
            let doc = tab_doc
                .get(tab_id)
                .copied()
                .unwrap_or_else(|| lunco_doc::DocumentId::new(0));
            (*tab_id, doc, state)
        })
    }
}

/// Cancel any in-flight projection task on a tab that isn't the
/// active tab.
///
/// On wasm, `AsyncComputeTaskPool` runs cooperatively on the main
/// thread. A projection that started for a tab the user has since
/// navigated away from keeps running through every `should_stop()`
/// check (which returns false because nobody flips its `cancel`
/// flag) all the way to completion, burning main-thread cycles the
/// active tab needs. This system flips the flag on every non-active
/// tab each Update tick — the running task short-circuits at its
/// next yield point and returns an empty Scene we'll discard.
///
/// Native: harmless. Real worker threads make the cooperative cancel
/// less critical (the projection still runs to completion off-thread)
/// but throwing away its result earlier saves a tiny amount of work.
pub fn cancel_inactive_projections(
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut state: ResMut<CanvasDiagramState>,
) {
    let active = workspace.as_deref().and_then(|ws| ws.active_document);
    for (_tab_id, doc_id, ds) in state.iter_mut() {
        // Cancel projections on any tab whose doc isn't the active
        // doc. We don't have a "currently focused tab" pointer, but
        // doc-level matching is conservative enough: same-doc tabs
        // (splits) keep going so the focused split's neighbours stay
        // current.
        if Some(doc_id) == active {
            continue;
        }
        if let Some(t) = ds.projection_task.as_ref() {
            t.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

// ─── Animation: pending API-focus queue ────────────────────────────────
//
// Implements the `OpOrigin::Api` half of `docs/architecture/20-domain-modelica.md`
// § 9c.5 (batch focus debounce). When an API caller adds a component,
// the API command observer writes a `PendingApiFocus` entry; this
// canvas-side system polls it each frame and, once the named node has
// landed in the projected scene, calls `viewport.set_target` to ease
// the camera onto it. The viewport's built-in tween (`set_target` +
// `tick` in `lunco-canvas/viewport.rs`) handles the actual smoothing
// — there is no separate animation system here.
//
// Why a queue rather than a direct-call observer: `AddComponent`
// applies synchronously, but the canvas reprojects asynchronously
// (off-thread parse → `projection_task`), so the new node isn't in
// `scene` for one or more frames. The queue waits patiently and
// applies the focus the moment the node appears.
//
// Batch debounce: when the queue contains multiple entries within a
// `BATCH_WINDOW`, the focus collapses to a single FitVisible over the
// accumulated set instead of ping-ponging between centroids — so a
// scripted N-component build animates into a single framed shot at
// the end. See § 9c.5 for the full rationale.
//
// TODO(modelica.canvas.add.focus_behavior): make None / Center /
// FitVisible settings-driven. Today the policy is hardcoded:
// single-add → Center, batch → FitVisible.
//
// TODO(modelica.canvas.add.batch_debounce_ms): expose `BATCH_WINDOW`
// as a setting.
//
// Pulse glow (§ 9c.4): the focus driver below also pushes matched
// (NodeId, started_at) pairs into the per-doc `PulseHandle`. The
// `PulseGlowLayer` (registered last in the canvas's layer list, so it
// paints on top) walks those entries each frame and draws a soft
// outer ring with alpha decaying linearly over `PULSE_DURATION`.
//
// TODO(modelica.canvas.animation.pulse_ms): expose `PULSE_DURATION`
// as a setting (0 = disable). Today it's hardcoded to 1.0 s.





/// Snapshot of a right-click: where to anchor the popup + what it
/// was targeted at. Close handling is done via egui's
/// `clicked_elsewhere()` on the popup's Response — no manual timer.
#[derive(Debug, Clone)]
pub struct PendingContextMenu {
    pub screen_pos: egui::Pos2,
    /// World position at click time — used when an "Add component"
    /// entry is selected so the new component lands where the user
    /// right-clicked, not at (0,0).
    pub world_pos: lunco_canvas::Pos,
    pub target: ContextMenuTarget,
}

#[derive(Debug, Clone)]
pub enum ContextMenuTarget {
    Node(lunco_canvas::NodeId),
    /// Right-click on an edge. `hit` classifies which part of the wire
    /// the click landed on — Body for plain wire body (delete /
    /// properties / etc.), Corner for an interior waypoint handle
    /// (delete bend), Segment for a segment midpoint (insert bend).
    /// `world_pos` is the click position in world coords; the
    /// insert-bend handler uses it as the new waypoint's location.
    Edge(lunco_canvas::EdgeId, lunco_canvas::EdgeHitKind),
    Empty,
}


// ─── Panel ─────────────────────────────────────────────────────────


// ─── MSL package tree (for nested add-component menu) ──────────────


// ─── Context-menu renderers ────────────────────────────────────────


/// Shorthand used by free helpers that don't already have the
/// active doc threaded through.
///
/// Prefers the per-render-call [`TabRenderContext`](crate::model_tabs_types::TabRenderContext)
/// so canvas bodies on a split see their own tab, then falls back
/// to the workspace-wide focused tab. Code paths that aren't part
/// of a tab body render (event observers, side-panel systems) hit
/// the fallback.
pub fn active_doc_from_world(world: &World) -> Option<lunco_doc::DocumentId> {
    if let Some((doc, _)) = world
        .get_resource::<crate::model_tabs_types::TabRenderContext>()
        .and_then(|c| c.current())
    {
        return Some(doc);
    }
    world
        .resource::<lunco_workspace::WorkspaceResource>()
        .active_document
}

/// `PanelCtx` sibling of [`active_doc_from_world`] — same precedence,
/// reading resources through the capability-narrowed panel context.
pub fn active_doc_from_world_ctx(
    ctx: &lunco_workbench::PanelCtx,
) -> Option<lunco_doc::DocumentId> {
    if let Some((doc, _)) = ctx
        .resource::<crate::model_tabs_types::TabRenderContext>()
        .and_then(|c| c.current())
    {
        return Some(doc);
    }
    ctx.resource::<lunco_workspace::WorkspaceResource>()
        .and_then(|w| w.active_document)
}

/// `(doc, drilled_class)` for the currently-rendering tab body, read through the
/// capability-narrowed [`PanelCtx`](lunco_workbench::PanelCtx). Falls back to
/// `(active_document, DrilledInClassNames[doc])` when no tab body is rendering.
#[cfg(feature = "ui")]
pub(super) fn render_target_ctx(
    ctx: &lunco_workbench::PanelCtx,
) -> Option<(lunco_doc::DocumentId, Option<String>)> {
    if let Some(tc) = ctx.resource::<crate::model_tabs_types::TabRenderContext>() {
        if let Some(doc) = tc.doc {
            return Some((doc, tc.drilled_class.clone()));
        }
    }
    let doc = ctx
        .resource::<lunco_workspace::WorkspaceResource>()
        .and_then(|w| w.active_document)?;
    let drilled = crate::sim_default::drilled_class_for_doc_ctx(ctx, doc);
    Some((doc, drilled))
}

/// Insert a plot scene node anchored at `click_world`. `entity_bits = 0`
/// + empty `signal_path` is the unbound form — the visual draws an
/// empty card the user can resize and bind later from the inspector.

// ─── Drill-in loading overlay ──────────────────────────────────────



/// SI unit suffix for the most common `Modelica.Units.SI.*` types used
/// by MSL Mechanics / Electrical / Blocks. Returned string is appended
/// to `%paramName` substitutions so the canvas matches OMEdit's
/// "value + unit" presentation (`J=2 kg.m2`, `c=1e4 N.m/rad`, …).
///
/// TODO: replace with proper type resolution. The authoritative source
/// is the type's declaration — `type Torque = Real(unit="N.m")` — not a
/// hand-maintained table. Plumb `unit` through `msl_indexer` (resolve
/// `comp.type_name` via scope chain + `class_cache`, walk the
/// `extends Real(unit=...)` modification) so `ParamDef.unit` is
/// populated from source. Once that lands, drop this fn and read
/// `p.unit` directly. Stopgap covers the high-frequency MSL types so
/// the PID example matches OMEdit; user-defined SI types (e.g.
/// `type Pressure = Real(unit="Pa")` in user models) fall through to
/// the bare value until the proper resolver is in.
pub(super) fn si_unit_suffix(param_type: &str) -> Option<&'static str> {
    let leaf = param_type.rsplit('.').next().unwrap_or(param_type);
    Some(match leaf {
        "Torque" => "N.m",
        "Inertia" => "kg.m2",
        "Mass" => "kg",
        "Length" => "m",
        "Distance" => "m",
        "Time" => "s",
        "Angle" => "rad",
        "AngularVelocity" => "rad/s",
        "AngularAcceleration" => "rad/s2",
        "Velocity" => "m/s",
        "Acceleration" => "m/s2",
        "Force" => "N",
        "Power" => "W",
        "Energy" => "J",
        "Frequency" => "Hz",
        "Temperature" | "ThermodynamicTemperature" => "K",
        "Voltage" => "V",
        "Current" => "A",
        "Resistance" => "Ohm",
        "Capacitance" => "F",
        "Inductance" => "H",
        "RotationalSpringConstant" | "TranslationalSpringConstant" => "N.m/rad",
        "RotationalDampingConstant" | "TranslationalDampingConstant" => "N.m.s/rad",
        _ => return None,
    })
}



// ─── Drill-in ───────────────────────────────────────────────────────


/// User-facing canvas snap settings, read each frame by the canvas
/// render path and pushed onto [`lunco_canvas::Canvas::snap`]. Off by
/// default; the Settings menu flips `enabled` and picks the step.
///
/// Step is in Modelica world units (not screen pixels) so the visible
/// grid spacing stays constant across zooms. Typical choices for the
/// standard `{{-100,-100},{100,100}}` diagram coord system:
///   * `2` — fine (matches common MSL placement granularity)
///   * `5` — medium
///   * `10` — coarse (matches typical integer placements in MSL)
#[derive(bevy::prelude::Resource)]
pub struct CanvasSnapSettings {
    pub enabled: bool,
    pub step: f32,
}

impl Default for CanvasSnapSettings {
    fn default() -> Self {
        // On by default. Step = 5 Modelica units — the OMEdit
        // default and the value most MSL example placements are
        // authored to (common placement extents are multiples of 5
        // or 10). Fine enough to reach typical target positions,
        // coarse enough that every drag produces a visibly
        // different "tick" as the icon crosses grid lines.
        Self {
            enabled: true,
            step: 5.0,
        }
    }
}












// ─── Doc-op translation ─────────────────────────────────────────────

