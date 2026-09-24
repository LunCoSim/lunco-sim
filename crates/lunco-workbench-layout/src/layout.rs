//! Dock layout state and perspective materialization for the workbench shell.
//!
//! The layout resource is kept separate from the shell's menu, status, and
//! rendering code so dock changes invalidate only this module during an
//! incremental build.

use crate::{find_leaf_matching, first_leaf, heal_non_finite_nulls, sanitize_dock_fractions};
use bevy::log::warn;
use bevy::prelude::{Resource, World};
use egui_dock::{DockState, NodeIndex};
use lunco_workbench_core::{
    InstancePanel, Panel, PanelId, PanelRenderTarget, PanelSlot, Perspective, PerspectiveId,
    PerspectiveLayoutPlan, TabId,
};
use lunco_workbench_state::{PerspectiveDockSnapshot, WorkspaceStateLayoutProvider};
use std::collections::HashMap;

/// Workbench state: registered panels plus the concrete dock tree.
///
/// Holds an `egui_dock::DockState<TabId>` plus registries of the panel
/// contracts keyed by their stable ids. The tree is mutated directly by the
/// user via egui_dock's drag-and-drop UI; perspectives return a
/// `PerspectiveLayoutPlan` that the shell materializes here.
#[derive(Resource)]
pub struct WorkbenchLayout {
    pub panels: HashMap<PanelId, Box<dyn Panel>>,
    /// Registered multi-instance panel kinds (one entry per
    /// [`InstancePanel::kind`]). Instances share the same renderer;
    /// each tab picks its behaviour via `TabId::Instance { kind, … }`.
    pub instance_panels: HashMap<PanelId, Box<dyn InstancePanel>>,
    pub perspectives: Vec<Box<dyn Perspective>>,
    pub active_perspective: Option<PerspectiveId>,
    /// Presentation-owned perspective required by an active guided flow.
    ///
    /// A guided may point at view-local `HelpAnchors`. While that flow is
    /// active, switching to a perspective that cannot publish those anchors
    /// would turn an ordinary user action into a guided failure. The
    /// guided sets this at its launch boundary and clears it when the flow
    /// ends; every perspective entry point is then constrained in one place.
    required_perspective: Option<String>,
    pub activity_bar: bool,

    // Slot intent — kept so perspectives can rebuild the dock when activated.
    // User drags after that mutate `dock` directly; intent goes stale until
    // the next perspective activation. Each side slot is a Vec so multiple
    // panels can be tabbed in the same dock region. The secondary vectors
    // describe the optional lower leaf used by the split Build layout.
    pub side_browser: Vec<PanelId>,
    pub side_browser_bottom: Vec<PanelId>,
    pub center: Vec<PanelId>,
    pub active_center_tab: usize,
    pub right_inspector: Vec<PanelId>,
    pub right_inspector_bottom: Vec<PanelId>,
    pub bottom: Vec<PanelId>,

    /// The live dock tree — what egui_dock actually renders. Stores
    /// [`TabId`]s so both singleton panels and multi-instance tabs
    /// coexist in the same tree.
    pub dock: DockState<TabId>,

    /// Per-perspective snapshots of the live dock + slot intent, so
    /// switching back to a perspective restores *its own* open tabs and
    /// split layout instead of a fresh preset (or the tabs another
    /// perspective left open). Keyed by [`PerspectiveId`]. A perspective
    /// is snapshotted on the way out (see [`Self::activate_perspective`])
    /// and restored on the way back; a first visit has no entry, so the
    /// perspective's preset is built fresh. This is what keeps Build's
    /// tabs and Design's tabs separate: each lives in its own dock tree.
    pub dock_cache: HashMap<PerspectiveId, PerspectiveDockSlot>,
}

/// Cached snapshot of one perspective's dock tree + slot intent — the
/// unit [`WorkbenchLayout::dock_cache`] stores per perspective so a
/// return visit restores the exact layout (tabs + splits + which centre
/// tab was active) the user left, rather than the preset.
#[derive(Clone)]
pub struct PerspectiveDockSlot {
    pub dock: DockState<TabId>,
    pub side_browser: Vec<PanelId>,
    pub side_browser_bottom: Vec<PanelId>,
    pub center: Vec<PanelId>,
    pub active_center_tab: usize,
    pub right_inspector: Vec<PanelId>,
    pub right_inspector_bottom: Vec<PanelId>,
    pub bottom: Vec<PanelId>,
}

impl Default for WorkbenchLayout {
    fn default() -> Self {
        Self {
            panels: HashMap::new(),
            instance_panels: HashMap::new(),
            perspectives: Vec::new(),
            active_perspective: None,
            required_perspective: None,
            activity_bar: false,
            side_browser: Vec::new(),
            side_browser_bottom: Vec::new(),
            center: Vec::new(),
            active_center_tab: 0,
            right_inspector: Vec::new(),
            right_inspector_bottom: Vec::new(),
            bottom: Vec::new(),
            dock: DockState::new(Vec::new()),
            dock_cache: HashMap::new(),
        }
    }
}

impl WorkbenchLayout {
    /// Register a panel and make its renderer available to the workbench.
    ///
    /// Before the first perspective is active, [`Panel::default_slot`] seeds
    /// the initial slot intent. Once a perspective is active, that
    /// perspective owns the slot intent; late registration must not add a
    /// panel to the active layout. A perspective that wants a late-registered
    /// panel declares its id through its `PerspectiveLayoutPlan`, and the
    /// rebuild below then realizes that declaration.
    pub fn register<P: Panel + 'static>(&mut self, panel: P) {
        self.register_boxed(Box::new(panel));
    }

    pub fn register_boxed(&mut self, panel: Box<dyn Panel>) {
        let id = panel.id();
        let slot = panel.default_slot();
        // A perspective may declare a panel before the domain plugin registers
        // its renderer. In that case the declared slot is authoritative: do
        // not append the panel to its authored default as well, or a stacked
        // Build preset silently turns back into one tab strip.
        let declared = self.side_browser.contains(&id)
            || self.side_browser_bottom.contains(&id)
            || self.center.contains(&id)
            || self.right_inspector.contains(&id)
            || self.right_inspector_bottom.contains(&id)
            || self.bottom.contains(&id);
        if self.active_perspective.is_none() && !declared {
            match slot {
                PanelSlot::SideBrowser => {
                    if !self.side_browser.contains(&id) {
                        self.side_browser.push(id);
                    }
                }
                PanelSlot::Center => {
                    if !self.center.contains(&id) {
                        self.center.push(id);
                    }
                }
                PanelSlot::RightInspector => {
                    if !self.right_inspector.contains(&id) {
                        self.right_inspector.push(id);
                    }
                }
                PanelSlot::Bottom => {
                    if !self.bottom.contains(&id) {
                        self.bottom.push(id);
                    }
                }
                PanelSlot::Hidden => { /* registered, intentionally not docked */ }
            }
        }
        self.panels.insert(id, panel);
        self.rebuild_dock();
    }

    /// Register a multi-instance panel *kind*. Tabs of this kind are
    /// opened via [`open_instance`](Self::open_instance) and dispatched
    /// to this [`InstancePanel`] by the workbench's tab viewer.
    ///
    /// A given kind should only be registered once per App; re-registering
    /// replaces the previous renderer.
    pub fn register_instance_panel<P: InstancePanel + 'static>(&mut self, panel: P) {
        self.register_instance_panel_boxed(Box::new(panel));
    }

    pub fn register_instance_panel_boxed(&mut self, panel: Box<dyn InstancePanel>) {
        self.instance_panels.insert(panel.kind(), panel);
    }

    /// Open (or focus, if already open) a multi-instance tab of `kind`
    /// with the given `instance` discriminant. Slot comes from the
    /// kind's [`InstancePanel::default_slot`] on first open.
    ///
    /// The workbench scans the dock for an existing tab matching the
    /// id and focuses it if found; otherwise adds a new tab to the
    /// **center** leaf — identified by matching any singleton tab
    /// currently in the `center` slot intent.
    pub fn open_instance(&mut self, kind: PanelId, instance: u64) {
        self.open_instance_with_slot(kind, instance, None);
    }

    fn open_instance_with_slot(
        &mut self,
        kind: PanelId,
        instance: u64,
        slot_override: Option<PanelSlot>,
    ) {
        if !self.instance_panels.contains_key(&kind) {
            bevy::log::warn!(
                "open_instance: no InstancePanel registered for kind {:?}",
                kind
            );
            return;
        }
        let tab = TabId::Instance { kind, instance };
        // Already open? Focus it.
        if let Some(path) = self.dock.find_tab(&tab) {
            self.dock.set_focused_node_and_surface(path.node_path());
            if let Err(e) = self.dock.set_active_tab(path) {
                bevy::log::warn!(
                    "open_instance: could not foreground tab {kind:?}#{instance} \
                     at {path:?}: {e:?}"
                );
            }
            return;
        }

        // Find the center leaf. We identify it as the one containing
        // any tab whose `PanelId` is in our `center` slot intent —
        // or, failing that, any existing `TabId::Instance` of this
        // same `kind` (because instance tabs of a kind belong in its
        // `default_slot`, which for model views is Center).
        //
        // Falling back to "first leaf" was wrong: after split_left /
        // split_right / split_below, the tree's first leaf in walk
        // order is the left side panel, and new tabs landed inside
        // the Package Browser instead of the center.
        // Resolve the kind's preferred slot. New instance tabs should
        // land in the same dock area as their kind's defaults — e.g.
        // a `VizPanel` (Bottom) opened next to the singleton `Graphs`
        // tab, NOT in the Center alongside the model view.
        let preferred_slot =
            slot_override.or_else(|| self.instance_panels.get(&kind).map(|p| p.default_slot()));
        // Build the set of singleton PanelIds occupying each slot so
        // we can find a leaf hosting any of them.
        let slot_ids: std::collections::HashSet<PanelId> = match preferred_slot {
            Some(PanelSlot::Center) => self.center.iter().copied().collect(),
            Some(PanelSlot::Bottom) => self.bottom.iter().copied().collect(),
            Some(PanelSlot::SideBrowser) => self.side_browser.iter().copied().collect(),
            Some(PanelSlot::RightInspector) => self.right_inspector.iter().copied().collect(),
            _ => std::collections::HashSet::new(),
        };
        let center_ids: std::collections::HashSet<PanelId> = self.center.iter().copied().collect();
        // The full-window 3D scene's leaf is EXCLUSIVE: an instance tab must never
        // be appended into it. That leaf's `ViewportPanel` renders nothing (the 3D
        // camera paints full-window behind it) but its render is what records the
        // scene-vs-chrome pick leaf; a co-tenant tab foregrounded there blanks the
        // viewport and swallows every click (the "opening a Graph kills the Build
        // controls" bug). We exclude it below and, if it's the only candidate,
        // split a fresh leaf beneath it instead.
        let vp_leaf = self.scene_viewport_leaf();
        let target_leaf = {
            let main = self.dock.main_surface_mut();
            // Priority 1: leaf already hosting another instance of
            // this kind — keeps families together.
            find_leaf_matching(
                main,
                |t| matches!(*t, TabId::Instance { kind: k, .. } if k == kind),
            )
            // Priority 2: leaf hosting any singleton in the
            // kind's preferred slot.
            .or_else(|| {
                find_leaf_matching(main, |t| match *t {
                    TabId::Singleton(id) => slot_ids.contains(&id),
                    _ => false,
                })
            })
            // Priority 3: leaf hosting any Center singleton (the
            // historical fallback, kept so kinds with no
            // preferred slot still land somewhere visible).
            .or_else(|| {
                find_leaf_matching(main, |t| match *t {
                    TabId::Singleton(id) => center_ids.contains(&id),
                    _ => false,
                })
            })
            // Priority 4: any leaf at all, except that a kind whose
            // authoritative default is the Bottom dock must create a bottom
            // split when no bottom singleton has reserved one. Otherwise the
            // first leaf can be the left browser, making a default graph tab
            // appear in the wrong surface.
            .or_else(|| {
                if preferred_slot != Some(PanelSlot::Bottom) {
                    first_leaf(main)
                } else {
                    None
                }
            })
            // …but never the exclusive scene-viewport leaf.
            .filter(|n| Some(*n) != vp_leaf)
        };

        if let Some(leaf) = target_leaf {
            let main = self.dock.main_surface_mut();
            main[leaf].append_tab(tab);
            // Focus the just-appended tab.
            if let Some(count) = main[leaf].tabs_count().checked_sub(1) {
                if let Err(e) = main.set_active_tab(leaf, count) {
                    bevy::log::warn!(
                        "open_instance: appended {kind:?}#{instance} to leaf \
                         {leaf:?} but could not foreground it: {e:?}"
                    );
                }
            }
            // Focus the leaf/surface too so egui_dock foregrounds it.
            self.dock.set_focused_node_and_surface(egui_dock::NodePath {
                surface: egui_dock::SurfaceIndex::main(),
                node: leaf,
            });
        } else if let Some(vp) = vp_leaf {
            // Only the scene-viewport leaf is available (e.g. Build, whose Bottom
            // slot is empty). Split a fresh leaf BELOW the viewport (~30% tall) and
            // drop the tab there so the viewport keeps its own exclusive leaf.
            self.dock.main_surface_mut().split_below(vp, 0.7, vec![tab]);
            if let Some(path) = self.dock.find_tab(&tab) {
                self.dock.set_focused_node_and_surface(path.node_path());
                let _ = self.dock.set_active_tab(path);
            }
        } else {
            // Empty dock (e.g. 3D app with no center tabs). Seed a
            // single leaf with this tab so at least something shows.
            self.dock = DockState::new(vec![tab]);
        }
    }

    /// Open an instance tab without changing the user's current tab.
    pub fn open_instance_without_focus(
        &mut self,
        kind: PanelId,
        instance: u64,
        restore: Option<TabId>,
    ) {
        let previous = restore.or_else(|| self.focused_tab().cloned());
        self.open_instance(kind, instance);
        if let Some(previous) = previous {
            if let Some(path) = self.dock.find_tab(&previous) {
                self.dock.set_focused_node_and_surface(path.node_path());
                let _ = self.dock.set_active_tab(path);
            }
        }
    }

    /// Rewrite the side-browser and right-inspector split fractions
    /// so the panes occupy a fixed absolute pixel width regardless
    /// of the current window size. Driven by [`maintain_dock_widths`]
    /// on `WindowResized`.
    ///
    /// Relies on the dock topology that [`rebuild_dock`] produces:
    /// - if `side_browser` non-empty, root is the side-left split.
    /// - if `right_inspector` non-empty, the right-inspector split
    ///   is the previous root, i.e. at `NodeIndex(2)` when wrapped
    ///   by a side-left split, or at `NodeIndex(0)` otherwise.
    pub fn enforce_widths(&mut self, window_w: f32, side_px: f32, right_px: f32) {
        // Reject non-finite inputs up front: `f32::clamp` propagates NaN, so a
        // NaN px width would be written straight into a split fraction and
        // panic egui_dock's separator layout on the next frame.
        if !window_w.is_finite() || !side_px.is_finite() || !right_px.is_finite() {
            return;
        }
        let total_w = window_w.max(100.0);
        let has_side = !self.side_browser.is_empty();
        let has_right = !self.right_inspector.is_empty();
        if !has_side && !has_right {
            return;
        }
        // `main_surface_mut()` returns `&mut Surface<Tab>` which
        // derefs to the underlying `Tree` for indexing.
        let tree = self.dock.main_surface_mut();
        if tree.is_empty() {
            return;
        }

        // Side-browser split — the outermost (root) when present.
        if has_side {
            let f = (side_px / total_w).clamp(0.05, 0.5);
            if let egui_dock::Node::Horizontal(ref mut s) = tree[NodeIndex(0)] {
                s.fraction = f;
            }
        }

        // Right-inspector split — at NodeIndex(2) if it lives inside
        // the side-left wrap, else at root.
        if has_right {
            let parent_w = if has_side {
                (total_w - side_px).max(100.0)
            } else {
                total_w
            };
            let right_share = (right_px / parent_w).clamp(0.05, 0.5);
            let f = 1.0 - right_share;
            let idx = if has_side { NodeIndex(2) } else { NodeIndex(0) };
            if idx.0 < tree.len() {
                if let egui_dock::Node::Horizontal(ref mut s) = tree[idx] {
                    s.fraction = f;
                }
            }
        }
    }

    /// Close a multi-instance tab if present. Idempotent.
    pub fn close_instance(&mut self, kind: PanelId, instance: u64) {
        let tab = TabId::Instance { kind, instance };
        if let Some(pos) = self.dock.find_tab(&tab) {
            self.dock.remove_tab(pos);
        }
    }

    /// Materialize a renderer-independent perspective plan into the concrete
    /// dock tree. Only this shell method knows how semantic slots map to
    /// `egui_dock` nodes.
    pub fn apply_perspective_plan(&mut self, plan: PerspectiveLayoutPlan) {
        self.activity_bar = plan.activity_bar;
        self.side_browser = plan.side_browser.primary;
        self.side_browser_bottom = plan.side_browser.secondary;
        self.center = plan.center.primary;
        self.active_center_tab = plan
            .active_center_tab
            .unwrap_or(0)
            .min(self.center.len().saturating_sub(1));
        self.right_inspector = plan.right_inspector.primary;
        self.right_inspector_bottom = plan.right_inspector.secondary;
        self.bottom = plan.bottom.primary;
        self.rebuild_dock();
        for tab in plan.instance_tabs {
            self.open_instance_with_slot(tab.kind, tab.instance, Some(tab.slot));
        }
    }

    /// Register a perspective (named workbench layout). The first one
    /// registered becomes the active default.
    pub fn register_perspective<W: Perspective + 'static>(&mut self, perspective: W) {
        let id = perspective.id();
        let first = self.perspectives.is_empty();
        self.perspectives.push(Box::new(perspective));
        if first {
            self.activate_perspective(id);
        }
    }

    /// Switch to the named perspective. No-op if the id isn't registered.
    ///
    /// Each perspective keeps its **own** open tabs and split layout: on
    /// the way out the live dock (+ slot intent) is snapshotted into
    /// [`Self::dock_cache`], and on the way back a previously-snapshotted
    /// perspective is restored verbatim instead of being rebuilt from its
    /// preset. A first visit has no snapshot, so the preset is built fresh
    /// — and the live dock is cleared first so the new perspective doesn't
    /// inherit the outgoing one's tabs (the old "VSCode never closes
    /// editors" merge is what made Build show Design's tabs).
    pub fn activate_perspective(&mut self, id: PerspectiveId) {
        // Guided presentations own their authored chrome for the duration of
        // the flow. This applies equally to the title-bar switcher, the typed
        // API command, and internal callers because they all converge here.
        // An unknown/stale requirement is ignored so it cannot make ordinary
        // perspective switching unusable after a provider disappears.
        let id = self
            .required_perspective
            .as_deref()
            .and_then(|required| {
                self.perspectives
                    .iter()
                    .find(|perspective| perspective.id().as_str() == required)
                    .map(|perspective| perspective.id())
            })
            .unwrap_or(id);
        // Validate the target before mutating — an unknown id is a no-op
        // and must not snapshot/restore anything.
        if !self.perspectives.iter().any(|w| w.id() == id) {
            return;
        }
        let restores_cached_layout = self
            .perspectives
            .iter()
            .find(|w| w.id() == id)
            .is_some_and(|w| w.restores_cached_layout());
        let prev = self.active_perspective;
        let switching = prev != Some(id);

        // Snapshot the outgoing perspective's live layout on a real switch.
        if switching {
            if let Some(prev_id) = prev {
                self.snapshot_perspective(prev_id);
            }
            // Restore a visited perspective's cached layout verbatim — its
            // tabs and splits come back exactly as left, no preset rebuild.
            if restores_cached_layout {
                if let Some(slot) = self.dock_cache.remove(&id) {
                    self.restore_perspective(slot);
                    self.active_perspective = Some(id);
                    return;
                }
            } else {
                // A presentation workspace must never revive stale document tabs.
                self.dock_cache.remove(&id);
            }
            // First visit: drop the live dock so the incoming preset's
            // rebuild seeds an empty skeleton instead of merging the
            // outgoing perspective's instance tabs into it.
            self.dock = DockState::new(Vec::new());
        }

        // `ws.layout()` borrows the registry immutably, while materialization
        // mutates the concrete shell. Take the registry out for the call.
        let perspectives = std::mem::take(&mut self.perspectives);
        if let Some(ws) = perspectives.iter().find(|w| w.id() == id) {
            let plan = ws.layout();
            self.apply_perspective_plan(plan);
            self.active_perspective = Some(id);
        }
        self.perspectives = perspectives;
    }

    /// Snapshot the current live dock + slot intent under `id` so a later
    /// return to that perspective restores it. No-op as a storage detail —
    /// the caller has already decided to switch away.
    fn snapshot_perspective(&mut self, id: PerspectiveId) {
        self.dock_cache.insert(
            id,
            PerspectiveDockSlot {
                dock: self.dock.clone(),
                side_browser: self.side_browser.clone(),
                side_browser_bottom: self.side_browser_bottom.clone(),
                center: self.center.clone(),
                active_center_tab: self.active_center_tab,
                right_inspector: self.right_inspector.clone(),
                right_inspector_bottom: self.right_inspector_bottom.clone(),
                bottom: self.bottom.clone(),
            },
        );
    }

    /// Restore a previously-snapshotted perspective's dock + slot intent
    /// into the live fields. The cached tree already carries its own
    /// chrome and tabs, so no rebuild is needed (and none must run, or it
    /// would wipe the restored tabs).
    fn restore_perspective(&mut self, slot: PerspectiveDockSlot) {
        let PerspectiveDockSlot {
            dock,
            side_browser,
            side_browser_bottom,
            center,
            active_center_tab,
            right_inspector,
            right_inspector_bottom,
            bottom,
        } = slot;
        self.dock = dock;
        self.side_browser = side_browser;
        self.side_browser_bottom = side_browser_bottom;
        self.center = center;
        self.active_center_tab = active_center_tab;
        self.right_inspector = right_inspector;
        self.right_inspector_bottom = right_inspector_bottom;
        self.bottom = bottom;
    }

    /// Which perspective is currently active, if any.
    pub fn active_perspective(&self) -> Option<PerspectiveId> {
        self.active_perspective
    }

    /// Require a registered perspective while a presentation owns the
    /// workbench. Passing `None` returns perspective selection to the user.
    ///
    /// The value is intentionally a runtime string: curriculum providers
    /// author perspective ids, while [`PerspectiveId`] is a static registry
    /// key. The requirement is resolved against the registered perspectives
    /// at activation time and never persisted as workspace state.
    pub fn set_required_perspective(&mut self, id: Option<&str>) {
        self.required_perspective = id.map(str::to_owned);
    }

    /// Reset the dock to a clean state by re-applying the active perspective's
    /// slot preset from scratch (or the first-registered perspective if none is
    /// active). Restores panels a stale persisted layout dropped — most
    /// importantly the 3D `ViewportPanel`, whose absence leaves the centre blank
    /// and the viewport camera inactive. Exposed as the `ResetWorkspaceLayout`
    /// command and the View ▸ "Reset Layout" menu item.
    pub fn reset_to_default_layout(&mut self) {
        let id = self
            .active_perspective
            .or_else(|| self.perspectives.first().map(|p| p.id()));
        if let Some(id) = id {
            // Drop any cached layout for this perspective and clear the live
            // dock so the re-activation rebuilds a *fresh* preset — without
            // this, `activate_perspective` would either restore the cached
            // (still-dirty) layout or preserve the current tabs.
            self.dock_cache.remove(&id);
            self.dock = DockState::new(Vec::new());
            self.activate_perspective(id);
        }
    }

    /// Reset the entire workbench presentation to its first-registered
    /// perspective and its authored slot preset. This is stronger than
    /// [`Self::reset_to_default_layout`]: opening a guided guided must not
    /// inherit the user's current perspective or any cached per-perspective
    /// tabs and splits.
    pub fn reset_to_default_perspective(&mut self) {
        let Some(id) = self
            .required_perspective
            .as_deref()
            .and_then(|required| {
                self.perspectives
                    .iter()
                    .find(|perspective| perspective.id().as_str() == required)
                    .map(|perspective| perspective.id())
            })
            .or_else(|| self.perspectives.first().map(|p| p.id()))
        else {
            return;
        };
        self.dock_cache.clear();
        self.dock = DockState::new(Vec::new());
        self.active_perspective = None;
        self.activate_perspective(id);
    }

    /// The tab selected in the focused leaf of the main dock surface.
    ///
    /// Perspective changes can briefly leave a focused node index pointing
    /// outside the rebuilt tree. Resolve the index through the tree iterator
    /// so that an empty or transitional dock has no focused tab instead of
    /// panicking while the shell publishes its snapshot.
    pub fn focused_tab(&self) -> Option<&TabId> {
        let tree = self.dock.main_surface();
        let focused = tree.focused_leaf()?;
        match tree.iter().nth(focused.0)? {
            egui_dock::Node::Leaf(leaf) => leaf.tabs.get(leaf.active.0),
            _ => None,
        }
    }

    /// The `instance` discriminant of the currently *focused* tab, when
    /// it's a multi-instance tab. Document tabs open with their
    /// `DocumentId.raw()` as the instance (see `open_instance` callers),
    /// so for a focused document this is the active document's id.
    ///
    /// The dock's focused leaf is the source of truth for which tab is
    /// active — `WorkspaceResource.active_document` isn't set on every
    /// open path, so reading it here is what makes hot-exit restore the
    /// *correct* active tab. Returns `None` when the focused tab is a
    /// singleton panel (not a document) or nothing is focused.
    pub fn active_tab_instance(&self) -> Option<u64> {
        if let Some(TabId::Instance { instance, .. }) = self.focused_tab() {
            return Some(*instance);
        }
        None
    }

    /// Serialize the live dock tree (split sizes, tab arrangement, active
    /// leaf) to JSON for per-Twin hot-exit. `TabId`/`PanelId` carry serde
    /// impls (`panel.rs`); the egui_dock `serde` feature does the rest.
    /// Returns `None` if serialization fails (never expected).
    pub fn dock_json(&self) -> Option<serde_json::Value> {
        serde_json::to_value(&self.dock).ok()
    }

    /// Cheap structural hash of the dock layout for in-memory change
    /// detection — folds tab arrangement, focus, split fractions and
    /// collapse state straight into a `u64` with no serialization.
    ///
    /// The workspace hot-exit gate runs every frame; it previously
    /// serialized the whole dock to a `serde_json::Value` + `String`
    /// purely to fold into this number — JSON is an I/O-boundary tool, not
    /// the right hammer for an internal change signal (CQ-209). This walks
    /// the live `DockState` nodes with a `Hasher` instead: zero
    /// allocations, and it deliberately ignores node `rect`s — those are
    /// recomputed from the window each layout pass and aren't persisted
    /// intent, so hashing them (as the JSON did) re-fired the save on every
    /// window resize.
    pub fn dock_layout_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for (path, node) in self.dock.iter_all_nodes() {
            path.surface.0.hash(&mut h);
            path.node.0.hash(&mut h);
            match node {
                egui_dock::Node::Empty => 0u8.hash(&mut h),
                egui_dock::Node::Leaf(leaf) => {
                    1u8.hash(&mut h);
                    leaf.tabs.len().hash(&mut h);
                    for tab in &leaf.tabs {
                        tab.hash(&mut h);
                    }
                    leaf.active.0.hash(&mut h);
                    leaf.collapsed.hash(&mut h);
                }
                egui_dock::Node::Vertical(s) => {
                    2u8.hash(&mut h);
                    s.fraction.to_bits().hash(&mut h);
                    s.fully_collapsed.hash(&mut h);
                }
                egui_dock::Node::Horizontal(s) => {
                    3u8.hash(&mut h);
                    s.fraction.to_bits().hash(&mut h);
                    s.fully_collapsed.hash(&mut h);
                }
            }
        }
        h.finish()
    }

    /// Reconcile a serialized dock tree against *this* app's live state,
    /// returning a fresh [`DockState<TabId>`] without touching the live
    /// dock:
    ///
    /// - **Singleton** tabs whose `PanelId` isn't registered here are
    ///   dropped (e.g. a `luncosim`-only panel loaded into `lunica`).
    /// - **Instance** tabs are remapped: each carries the *old* session's
    ///   instance id; `id_map` translates it to the freshly-restored id.
    ///   A tab whose kind isn't registered is dropped; one whose document
    ///   didn't restore (absent from `id_map`) is kept unless its kind is in
    ///   `discard_unmapped_kinds`. Stable-instance tabs like the default plot
    ///   stay open; stale document-backed tabs can be removed.
    ///
    /// Empty leaves collapse via egui_dock's `retain_tabs`; non-finite
    /// split fractions are healed ([`sanitize_dock_fractions`]). Returns
    /// `None` when the JSON won't parse or nothing survived — the caller
    /// then keeps its current dock. Shared by the live restore path
    /// ([`set_dock_from_json`]) and the per-perspective cache seeding
    /// ([`seed_perspective_docks`]) so cached trees go through the same
    /// cross-app reconciliation as the active one.
    pub fn reconcile_dock(
        &self,
        value: serde_json::Value,
        id_map: &HashMap<(&'static str, u64), u64>,
        discard_unmapped_kinds: &std::collections::HashSet<&'static str>,
    ) -> Option<DockState<TabId>> {
        use std::collections::HashSet;
        let valid_singletons: HashSet<&'static str> = self.panels.keys().map(|p| p.0).collect();
        let valid_kinds: HashSet<&'static str> = self.instance_panels.keys().map(|p| p.0).collect();

        // A NaN fraction (see `sanitize_dock_fractions`) serializes to JSON
        // `null`, which won't deserialize back into `f32` — so the load-time
        // sanitize below is unreachable unless we heal the `Value` first.
        let mut value = value;
        heal_non_finite_nulls(&mut value);

        let mut new_dock: DockState<TabId> = match serde_json::from_value(value) {
            Ok(d) => d,
            Err(e) => {
                warn!("[WorkspaceState] dock JSON parse failed: {e}; keeping default layout");
                return None;
            }
        };

        // One pass: drop unregistered-kind tabs, remap restored instances,
        // preserve unmatched stable-instance tabs, and discard unmatched
        // document-backed tabs when their codec requests that behavior.
        // A singleton panel is one renderer with one egui identity. A stale
        // layout may contain it in more than one leaf (older Build layouts put
        // Telemetry in both side and bottom), which makes egui render the same
        // widget tree twice and report ID collisions. Keep the first occurrence
        // in dock order; the current perspective then supplies its canonical
        // position on the next normal layout rebuild.
        let mut seen_singletons = HashSet::new();
        new_dock.retain_tabs(|tab| match tab {
            TabId::Singleton(pid) => {
                valid_singletons.contains(pid.0) && seen_singletons.insert(*pid)
            }
            TabId::Instance { kind, instance } => {
                if !valid_kinds.contains(kind.0) {
                    return false;
                }
                if let Some(&new_id) = id_map.get(&(kind.0, *instance)) {
                    *instance = new_id;
                } else if discard_unmapped_kinds.contains(kind.0) {
                    return false;
                }
                true
            }
        });

        new_dock.iter_all_tabs().next()?;
        // Heal any non-finite split fraction persisted to disk. egui_dock can
        // serialize a NaN fraction (see `sanitize_dock_fractions`), and a NaN
        // reloaded here would panic the dock layout on the very next frame —
        // a permanent boot-crash loop until the workspace cache is wiped.
        sanitize_dock_fractions(&mut new_dock);
        // Heal a blanked 3D viewport. The full-window scene `ViewportPanel`
        // (`scene_target() == MainViewport`) renders nothing itself — the 3D camera
        // paints full-window behind it — but its `render` is what records the
        // scene-vs-chrome pick leaf. A persisted layout (or a stray drag) can leave
        // another tab (e.g. a Modelica plot) tabbed onto the viewport's leaf;
        // foregrounded there it blanks the viewport, which then never records its
        // leaf, and every click in the still-visible 3D is swallowed as chrome (the
        // "3D visible but the middle of Build is dead" regression). Evict any such
        // co-tenant into its own leaf so the viewport is alone and live again.
        if let Some(vp_id) = self.scene_viewport_panel_id() {
            Self::evict_scene_viewport_cotenants(&mut new_dock, TabId::Singleton(vp_id));
        }
        Some(new_dock)
    }

    /// The registered singleton panel that IS the full-window 3D scene
    /// (`Panel::scene_target() == Some(lunco_workbench_core::scene_pick::SceneTarget::MainViewport)`), if any.
    /// App-agnostic — every workbench app that hosts a 3D scene registers exactly
    /// one such panel (the luncosim's `ViewportPanel`); tooling apps register none.
    /// Used to keep that panel foregrounded so a co-tenant tab can never blank the
    /// viewport controls (see [`reconcile_dock`](Self::reconcile_dock)).
    fn scene_viewport_panel_id(&self) -> Option<PanelId> {
        self.panels
            .iter()
            .find(|(_, p)| p.scene_target() == Some(PanelRenderTarget::MainViewport))
            .map(|(id, _)| *id)
    }

    /// The dock leaf currently hosting the scene [`ViewportPanel`], if it's in the
    /// tree. That leaf is kept EXCLUSIVE (see [`open_instance`](Self::open_instance)
    /// and [`evict_scene_viewport_cotenants`](Self::evict_scene_viewport_cotenants))
    /// so no other tab can blank the 3D.
    fn scene_viewport_leaf(&self) -> Option<NodeIndex> {
        let vp = self.scene_viewport_panel_id()?;
        self.dock.find_tab(&TabId::Singleton(vp)).map(|p| p.node)
    }

    /// Move any tab sharing the scene viewport's leaf out into a fresh leaf split
    /// below it, so the viewport ends up alone. A persisted layout (or a stray
    /// drag) can leave e.g. a Modelica plot tabbed onto the viewport; foregrounded,
    /// it blanks the 3D and eats clicks. Operates on `dock` in place; no-op when the
    /// viewport isn't present or already has its leaf to itself.
    fn evict_scene_viewport_cotenants(dock: &mut DockState<TabId>, vp_tab: TabId) {
        let Some(pos) = dock.find_tab(&vp_tab) else {
            return;
        };
        let vp_node = pos.node;
        let cotenants: Vec<TabId> = match &dock.main_surface()[vp_node] {
            egui_dock::Node::Leaf(leaf) => {
                leaf.tabs.iter().copied().filter(|t| *t != vp_tab).collect()
            }
            _ => Vec::new(),
        };
        if cotenants.is_empty() {
            // Viewport already alone — just make sure it's the active tab.
            let _ = dock.set_active_tab(pos);
            return;
        }
        let main = dock.main_surface_mut();
        if let egui_dock::Node::Leaf(leaf) = &mut main[vp_node] {
            leaf.tabs.retain(|t| *t == vp_tab);
            leaf.active = egui_dock::TabIndex(0);
        }
        // Fresh leaf beneath the viewport (~30% tall) holds the evicted tabs.
        main.split_below(vp_node, 0.7, cotenants);
    }

    /// Replace the live dock from a previously [`dock_json`](Self::dock_json)
    /// snapshot (reconciled via [`reconcile_dock`]). Returns `false`
    /// (leaving the current dock untouched) when the JSON won't parse or the
    /// reconciled tree would be empty — the caller then keeps whatever the
    /// codec-driven open path produced.
    pub fn set_dock_from_json(
        &mut self,
        value: serde_json::Value,
        id_map: &HashMap<(&'static str, u64), u64>,
        discard_unmapped_kinds: &std::collections::HashSet<&'static str>,
    ) -> bool {
        match self.reconcile_dock(value, id_map, discard_unmapped_kinds) {
            Some(d) => {
                self.dock = d;
                true
            }
            None => false,
        }
    }

    /// Capture every perspective's dock tree (+ slot intent) for hot-exit:
    /// each cached perspective's dock ([`dock_cache`]) plus the active
    /// perspective's **live** dock. Chrome-incomplete trees (a transient
    /// state mid-switch) are skipped so they don't round-trip as a layout
    /// with missing panels. Keyed by [`PerspectiveId`] string — the inverse
    /// of [`seed_perspective_docks`]. The active perspective is captured
    /// last so an id that's somehow both live and cached resolves to the
    /// live tree.
    pub fn capture_perspective_docks(
        &self,
    ) -> std::collections::HashMap<String, PerspectiveDockSnapshot> {
        let mut out: std::collections::HashMap<String, PerspectiveDockSnapshot> =
            std::collections::HashMap::new();
        for (id, slot) in &self.dock_cache {
            if !self.chrome_complete(
                &slot.dock,
                &slot.side_browser,
                &slot.side_browser_bottom,
                &slot.center,
                &slot.right_inspector,
                &slot.right_inspector_bottom,
                &slot.bottom,
            ) {
                continue;
            }
            let Some(dock) = serde_json::to_value(&slot.dock).ok() else {
                continue;
            };
            out.insert(
                id.as_str().to_string(),
                PerspectiveDockSnapshot {
                    layout_revision: self.perspective_layout_revision(*id),
                    dock,
                    side_browser: slot.side_browser.clone(),
                    side_browser_bottom: slot.side_browser_bottom.clone(),
                    center: slot.center.clone(),
                    active_center_tab: slot.active_center_tab,
                    right_inspector: slot.right_inspector.clone(),
                    right_inspector_bottom: slot.right_inspector_bottom.clone(),
                    bottom: slot.bottom.clone(),
                },
            );
        }
        if let Some(id) = self.active_perspective() {
            if self.perspective_chrome_complete() {
                if let Some(dock) = self.dock_json() {
                    out.insert(
                        id.as_str().to_string(),
                        PerspectiveDockSnapshot {
                            layout_revision: self.perspective_layout_revision(id),
                            dock,
                            side_browser: self.side_browser.clone(),
                            side_browser_bottom: self.side_browser_bottom.clone(),
                            center: self.center.clone(),
                            active_center_tab: self.active_center_tab,
                            right_inspector: self.right_inspector.clone(),
                            right_inspector_bottom: self.right_inspector_bottom.clone(),
                            bottom: self.bottom.clone(),
                        },
                    );
                }
            }
        }
        out
    }

    /// Restore every saved perspective's dock tree after a launch / Twin
    /// switch — the inverse of [`capture_perspective_docks`]. The **active**
    /// perspective's tree is reconciled into the live dock
    /// ([`set_dock_from_json`] + [`ensure_chrome_present`]); every other
    /// saved perspective is reconciled into a [`PerspectiveDockSlot`] and
    /// stashed in [`dock_cache`], so switching to it later restores its own
    /// tabs instead of a fresh preset. `id_map` remaps saved instance ids
    /// onto live tabs across ALL trees (instance ids are global — the doc
    /// set is opened once). Perspectives not registered in this app are
    /// skipped (a `luncosim`-only perspective loaded into `lunica`).
    pub fn seed_perspective_docks(
        &mut self,
        docks: &std::collections::HashMap<String, PerspectiveDockSnapshot>,
        id_map: &HashMap<(&'static str, u64), u64>,
        discard_unmapped_kinds: &std::collections::HashSet<&'static str>,
    ) {
        let active_str = self.active_perspective().map(|p| p.as_str().to_string());

        // Active perspective: reconcile into the LIVE dock + heal chrome.
        if let Some(active) = active_str.as_deref() {
            if let Some(snap) = docks.get(active).filter(|snap| {
                snap.layout_revision == self.perspective_layout_revision_by_str(active)
            }) {
                if self.set_dock_from_json(snap.dock.clone(), id_map, discard_unmapped_kinds) {
                    self.ensure_chrome_present();
                }
            }
        }

        // Non-active perspectives: reconcile each into a cached slot. Collect
        // first (reconcile borrows &self), insert after (borrows &mut self).
        let mut seeded: Vec<(PerspectiveId, PerspectiveDockSlot)> = Vec::new();
        for (id_str, snap) in docks {
            if Some(id_str.as_str()) == active_str.as_deref() {
                continue;
            }
            let Some(pid) = self
                .perspectives
                .iter()
                .find(|p| p.id().as_str() == id_str.as_str())
                .map(|p| p.id())
            else {
                continue; // not registered in this app — skip
            };
            if snap.layout_revision != self.perspective_layout_revision(pid) {
                continue;
            }
            let Some(slot) = self.reconcile_dock_slot(snap, id_map, discard_unmapped_kinds) else {
                continue;
            };
            seeded.push((pid, slot));
        }
        for (pid, slot) in seeded {
            self.dock_cache.insert(pid, slot);
        }
    }

    /// Return the registered preset revision for a perspective.
    fn perspective_layout_revision(&self, id: PerspectiveId) -> u32 {
        self.perspectives
            .iter()
            .find(|perspective| perspective.id() == id)
            .map_or(0, |perspective| perspective.layout_revision())
    }

    /// String-keyed companion used while restoring persisted state.
    fn perspective_layout_revision_by_str(&self, id: &str) -> u32 {
        self.perspectives
            .iter()
            .find(|perspective| perspective.id().as_str() == id)
            .map_or(0, |perspective| perspective.layout_revision())
    }

    /// Reconcile a [`PerspectiveDockSnapshot`] into a live
    /// [`PerspectiveDockSlot`] for the cache (reconciled tree + the slot
    /// intent carried alongside it). `None` when the dock won't reconcile —
    /// the caller skips caching that perspective (it builds from preset on
    /// first visit instead).
    fn reconcile_dock_slot(
        &self,
        snap: &PerspectiveDockSnapshot,
        id_map: &HashMap<(&'static str, u64), u64>,
        discard_unmapped_kinds: &std::collections::HashSet<&'static str>,
    ) -> Option<PerspectiveDockSlot> {
        let dock = self.reconcile_dock(snap.dock.clone(), id_map, discard_unmapped_kinds)?;
        Some(PerspectiveDockSlot {
            dock,
            side_browser: snap.side_browser.clone(),
            side_browser_bottom: snap.side_browser_bottom.clone(),
            center: snap.center.clone(),
            active_center_tab: snap.active_center_tab,
            right_inspector: snap.right_inspector.clone(),
            right_inspector_bottom: snap.right_inspector_bottom.clone(),
            bottom: snap.bottom.clone(),
        })
    }

    /// Activate a perspective by its raw string id, matching against the
    /// registered set. Returns `true` if a perspective with that id
    /// exists in this app and was activated; `false` (no-op) otherwise.
    ///
    /// The reconciliation seam for persisted state: a `PerspectiveId`
    /// holds a `&'static str` and can't be rebuilt from a runtime
    /// `String`, so restore looks the string up here and drops ids that
    /// aren't registered in the current binary (e.g. a perspective only
    /// `luncosim` ships, loaded into `lunica`).
    pub fn activate_perspective_by_str(&mut self, id: &str) -> bool {
        let found = self
            .perspectives
            .iter()
            .find(|p| p.id().as_str() == id)
            .map(|p| p.id());
        match found {
            Some(pid) => {
                self.activate_perspective(pid);
                true
            }
            None => false,
        }
    }

    /// Rebuild the dock tree from the current slot intent.
    ///
    /// Called by every slot setter and by [`activate_perspective`]. After
    /// rebuild, user drags persist until the next call.
    ///
    /// **Two-mode rendering** — the dock is only used when there are
    /// central tabs (i.e. apps like `lunica` that have
    /// Code/Diagram in the centre). In 3D apps where the centre is
    /// reserved for the Bevy viewport, the dock is left empty and the
    /// side panels render via plain `egui::SidePanel`/`TopBottomPanel`
    /// instead — see [`render_layout`]. This keeps the central region
    /// transparent so the 3D scene shows through.
    ///
    /// **`egui_dock` fraction quirk** — the docstring says `fraction`
    /// is the OLD node's share, but that's only true for `split_right`
    /// and `split_below`. For `split_left` and `split_above`, `fraction`
    /// is actually the NEW node's share, because the renderer places
    /// the divider at `rect.min + size * fraction` and the new node
    /// sits at `parent.left()` (i.e. the first child). So:
    ///
    /// | function | NEW gets | OLD gets |
    /// |---|---|---|
    /// | `split_left(_, f, new)` | `f` | `1 - f` |
    /// | `split_above(_, f, new)` | `f` | `1 - f` |
    /// | `split_right(_, f, new)` | `1 - f` | `f` |
    /// | `split_below(_, f, new)` | `1 - f` | `f` |
    ///
    /// We always pick the fraction so the panel we just added gets a
    /// small share (20% side, 22% right, 30% bottom).
    /// Insert a panel into the live dock without rebuilding from
    /// scratch. Used by the View menu's panel checkbox so toggling
    /// one tab doesn't wipe instance tabs (model views, etc.) that
    /// the perspective preset doesn't track. Picks a leaf based on
    /// the panel's default slot; falls back to the focused leaf.
    /// Returns true if the panel was inserted.
    pub fn insert_panel_into_dock(&mut self, id: PanelId, slot: PanelSlot) -> bool {
        let tab = TabId::Singleton(id);
        // Already there? No-op.
        if self.dock.iter_all_tabs().any(|(_, t)| *t == tab) {
            return false;
        }
        let main = self.dock.main_surface_mut();
        // Find an existing tab in the same slot to drop next to.
        let neighbour: Option<PanelId> = match slot {
            PanelSlot::SideBrowser => self.side_browser.first().copied(),
            PanelSlot::Center => self.center.first().copied(),
            PanelSlot::RightInspector => self.right_inspector.first().copied(),
            PanelSlot::Bottom => self.bottom.first().copied(),
            PanelSlot::Hidden => None,
        };
        let target_node: Option<NodeIndex> = neighbour.and_then(|nid| {
            let target_tab = TabId::Singleton(nid);
            // Walk all nodes; egui_dock's NodeIndex is opaque so we
            // probe by index until we find the leaf containing the
            // sibling tab.
            let mut found = None;
            for i in 0..256 {
                let node = NodeIndex(i);
                if let Some(node_ref) = main.iter().nth(i) {
                    if let egui_dock::Node::Leaf(leaf) = node_ref {
                        if leaf.tabs.contains(&target_tab) {
                            found = Some(node);
                            break;
                        }
                    }
                } else {
                    break;
                }
            }
            found
        });
        if let Some(node) = target_node {
            main.set_focused_node(node);
            main.push_to_focused_leaf(tab);
        } else {
            // Last resort: append to focused leaf (whatever the user
            // had focus on). Better than wiping the dock.
            main.push_to_focused_leaf(tab);
        }
        true
    }

    /// Activate (foreground) a singleton panel tab if it's already
    /// present in the dock. Returns `true` when the panel was found
    /// and focused, `false` when no leaf contains it. Idempotent —
    /// calling on the already-active tab is a no-op success.
    ///
    /// Used by the [`FocusPanel`] typed command so HTTP / scripting
    /// callers can deterministically bring a panel forward (e.g.
    /// activating Experiments before screenshotting it).
    pub fn focus_singleton(&mut self, id: PanelId) -> bool {
        let tab = TabId::Singleton(id);
        if let Some(pos) = self.dock.find_tab(&tab) {
            self.dock.set_focused_node_and_surface(pos.node_path());
            // A stale path here means the tab is present but unreachable — report
            // failure to the caller (the `FocusPanel` command) instead of silently
            // claiming success while nothing foregrounds.
            if let Err(e) = self.dock.set_active_tab(pos) {
                bevy::log::warn!("focus_singleton: could not foreground {id:?} at {pos:?}: {e:?}");
                return false;
            }
            true
        } else {
            false
        }
    }

    /// Remove a panel from the live dock without rebuilding from
    /// scratch. Companion to [`insert_panel_into_dock`].
    pub fn remove_panel_from_dock(&mut self, id: PanelId) -> bool {
        let tab = TabId::Singleton(id);
        let mut removed = false;
        let main = self.dock.main_surface_mut();
        // Collect node indices to mutate.
        let mut hits: Vec<(NodeIndex, usize)> = Vec::new();
        for i in 0..256 {
            let node = NodeIndex(i);
            match main.iter().nth(i) {
                Some(egui_dock::Node::Leaf(leaf)) => {
                    for (idx, t) in leaf.tabs.iter().enumerate() {
                        if *t == tab {
                            hits.push((node, idx));
                        }
                    }
                }
                Some(_) => {}
                None => break,
            }
        }
        for (node, idx) in hits.into_iter().rev() {
            if let Some(egui_dock::Node::Leaf(leaf)) = main.iter_mut().nth(node.0) {
                if idx < leaf.tabs.len() {
                    leaf.tabs.remove(idx);
                    removed = true;
                }
            }
        }
        removed
    }

    pub fn rebuild_dock(&mut self) {
        // Filter slot intent down to panels actually registered in this
        // app, so perspective presets can optimistically list panels that
        // may only exist in some binaries (e.g. a rover-only Code tab
        // referenced from the shared `BuildPerspective`).
        //
        // Perspective plans still use `PanelId` — slot declarations describe
        // singleton-panel layouts. Instance-panel tabs are
        // opened dynamically at runtime (e.g. Package Browser opens a
        // model tab) and don't come from the perspective preset.
        let known = |ids: &[PanelId]| -> Vec<TabId> {
            ids.iter()
                .copied()
                .filter(|id| self.panels.contains_key(id))
                .map(TabId::Singleton)
                .collect()
        };
        let side_browser_tabs = known(&self.side_browser);
        let side_browser_bottom_tabs = known(&self.side_browser_bottom);
        let right_inspector_tabs = known(&self.right_inspector);
        let right_inspector_bottom_tabs = known(&self.right_inspector_bottom);
        let bottom_tabs = known(&self.bottom);
        let center_tabs: Vec<TabId> = self
            .center
            .iter()
            .copied()
            .filter(|id| self.panels.contains_key(id))
            .map(TabId::Singleton)
            .collect();

        // Preserve dynamically-opened instance (document/model/viz) tabs
        // across a *same-perspective* slot rebuild (e.g. a panel re-registering
        // or `ResetWorkspaceLayout` after it
        // cleared the dock). The skeleton below is built purely from the
        // *singleton* slot intent, so without this every instance tab —
        // open model docs, plot instances — would silently vanish when a
        // slot intent changes within one perspective. VSCode never closes
        // open editors when you change the layout; neither do we. We walk
        // the current dock, remember each instance tab (in order) and
        // which one was focused, then re-attach them via `open_instance`
        // after the skeleton is rebuilt. Tabs whose kind is no longer
        // registered are dropped.
        //
        // Cross-perspective switches do NOT reach here with the outgoing
        // tabs still live: `activate_perspective` clears `self.dock`
        // before rebuilding the incoming perspective's preset (or restores
        // a cached dock without rebuilding at all), so each perspective's
        // tabs stay isolated in its own dock cache entry.
        let preserved_instances: Vec<(PanelId, u64)> = {
            let main = self.dock.main_surface();
            let mut acc = Vec::new();
            for node in main.iter() {
                if let egui_dock::Node::Leaf(leaf) = node {
                    for tab in &leaf.tabs {
                        if let TabId::Instance { kind, instance } = tab {
                            if self.instance_panels.contains_key(kind) {
                                acc.push((*kind, *instance));
                            }
                        }
                    }
                }
            }
            acc
        };
        let active_instance: Option<(PanelId, u64)> =
            self.focused_tab().and_then(|tab| match tab {
                TabId::Instance { kind, instance } if self.instance_panels.contains_key(kind) => {
                    Some((*kind, *instance))
                }
                _ => None,
            });

        // Viewport-only perspectives: no central singleton tabs → don't
        // build a side-panel dock tree. The renderer lays out side panels
        // with egui's SidePanels and leaves the central area transparent
        // (it stays in 3D mode — see the `has_dock_tabs` gate in
        // `render_layout` — so a non-empty dock here is *not* shown).
        //
        // A pure 3D app keeps no instance tabs at all, but a hybrid app
        // (the rover luncosim embeds the Modelica workbench) can have
        // document/model tabs open while a viewport-only perspective is
        // active. Park those instance tabs in the dock rather than dropping
        // them — wiping would lose the open documents on every viewport
        // perspective activation. They render nowhere while this
        // perspective is active and re-attach to the centre when the user
        // switches to a centre-driven perspective (which collects them as
        // `preserved_instances` on its own rebuild).
        if center_tabs.is_empty() {
            let parked: Vec<TabId> = preserved_instances
                .iter()
                .map(|(kind, instance)| TabId::Instance {
                    kind: *kind,
                    instance: *instance,
                })
                .collect();
            self.dock = DockState::new(parked);
            return;
        }

        // Centre-driven apps: build the standard cross layout in egui_dock.
        // Splits are ordered so right and left span the full window height,
        // and bottom spans the central column's width (sandwiched between
        // them). Each subsequent split at NodeIndex::root() wraps the
        // previous tree, so the outermost splits dominate the layout.
        let mut dock = DockState::new(center_tabs);
        let mut central = NodeIndex::root();

        if !bottom_tabs.is_empty() {
            let main = dock.main_surface_mut();
            let [center_after, _below] = main.split_below(central, 0.7, bottom_tabs);
            central = center_after;
        }

        // Target initial split: 15% side / 65% centre / 20% right.
        // Splits compound: split_right runs first, then split_left wraps
        // the whole tree and shrinks the previous splits proportionally.
        // To land at the target after compounding:
        //   split_right with f_right = 0.765 → right = (1 - 0.765) of pre-left-split = 0.235
        //   split_left  with f_left  = 0.15  → side = 0.15 of total
        //   Right after compounding  = 0.235 × (1 - 0.15) = 0.200 ✓
        //   Centre after compounding = 0.765 × (1 - 0.15) = 0.650 ✓
        if !right_inspector_tabs.is_empty() {
            let main = dock.main_surface_mut();
            let [_old_root, right] =
                main.split_right(NodeIndex::root(), 0.765, right_inspector_tabs);
            if !right_inspector_bottom_tabs.is_empty() {
                let [_top, _bottom] = main.split_below(right, 0.5, right_inspector_bottom_tabs);
            }
        }

        if !side_browser_tabs.is_empty() {
            let main = dock.main_surface_mut();
            // For split_left, fraction is the NEW (left) share — see
            // the table in the doc above. Bumped from 0.15 → 0.22 so
            // the Twin Browser shows full library names ("Modelica
            // Standard Library") without truncation at default zoom.
            let [_old_root, left] = main.split_left(NodeIndex::root(), 0.22, side_browser_tabs);
            if !side_browser_bottom_tabs.is_empty() {
                let [_top, _bottom] = main.split_below(left, 0.5, side_browser_bottom_tabs);
            }
        }

        let _ = central;
        self.dock = dock;

        // Re-attach the instance tabs we remembered above. `open_instance`
        // resolves each kind's preferred-slot leaf and appends there, so a
        // model doc lands back in the centre and a plot back in the bottom
        // — exactly where they were, even though the skeleton only knows
        // about singleton slots. It focuses each as it goes; we restore the
        // originally-focused instance tab last so the right one stays
        // active.
        for (kind, instance) in &preserved_instances {
            self.open_instance(*kind, *instance);
        }
        if let Some((kind, instance)) = active_instance {
            // Idempotent: the tab is already present, so this just
            // re-focuses it.
            self.open_instance(kind, instance);
        }
    }

    /// Reconcile a freshly-restored dock tree against the active
    /// perspective's declared chrome (side browser / inspectors /
    /// bottom singletons).
    ///
    /// A persisted dock can omit those panels — e.g. it was last saved
    /// while a viewport-only perspective was active (which parks only
    /// instance tabs, no chrome — see [`Self::rebuild_dock`]'s
    /// `center_tabs.is_empty()` branch), or from an older layout. In
    /// dock-mode the renderer draws the dock tree verbatim, so any
    /// missing chrome silently never appears (open documents show, but
    /// the side/right panels are gone).
    ///
    /// When the active perspective is centre-driven (it declares
    /// registered centre singletons) yet the restored dock is missing
    /// any declared chrome, rebuild the full layout from intent.
    /// [`Self::rebuild_dock`] re-attaches the open document/instance
    /// tabs, so only the saved split *sizes* are lost — not the open
    /// documents or the chrome. Viewport-only perspectives (no
    /// registered centre singleton — the luncosim's `View`) are left
    /// untouched: their chrome lives outside the dock by design.
    pub fn ensure_chrome_present(&mut self) {
        if !self.perspective_chrome_complete() {
            warn!(
                "[WorkspaceState] restored dock missing perspective chrome; \
                 rebuilding layout (open documents preserved, split sizes reset)"
            );
            self.rebuild_dock();
        }
    }

    /// True when the live dock is consistent with the active
    /// perspective's declared chrome. Either the perspective is
    /// viewport-only (declares no *registered* centre singleton — its
    /// side panels render outside the dock, so a chrome-less dock is
    /// correct), or every declared+registered chrome panel
    /// (side/right/bottom/centre singleton) is present in the dock tree.
    ///
    /// Used at both ends of persistence: [`Self::ensure_chrome_present`]
    /// heals a restored dock that fails this, and `build_state` refuses
    /// to persist a dock that fails it (so a transient chrome-less dock —
    /// e.g. mid perspective-switch through the viewport-only
    /// [`Self::rebuild_dock`] branch — never round-trips as a layout with
    /// missing side panels).
    pub fn perspective_chrome_complete(&self) -> bool {
        self.chrome_complete(
            &self.dock,
            &self.side_browser,
            &self.side_browser_bottom,
            &self.center,
            &self.right_inspector,
            &self.right_inspector_bottom,
            &self.bottom,
        )
    }

    /// True when `dock` is consistent with the given slot intent's declared
    /// chrome. Either the intent is viewport-only (declares no *registered*
    /// centre singleton — its chrome renders outside the dock, so a
    /// chrome-less dock is correct), or every declared+registered chrome
    /// panel (side/right/bottom/centre singleton) is present in `dock`.
    ///
    /// Parameterised over the dock + intent so it can judge a CACHED
    /// perspective's dock by its OWN intent (in
    /// [`capture_perspective_docks`]), not the active perspective's. The
    /// live variant is [`perspective_chrome_complete`].
    fn chrome_complete(
        &self,
        dock: &DockState<TabId>,
        side_browser: &[PanelId],
        side_browser_bottom: &[PanelId],
        center: &[PanelId],
        right_inspector: &[PanelId],
        right_inspector_bottom: &[PanelId],
        bottom: &[PanelId],
    ) -> bool {
        let is_centre_driven = center.iter().any(|id| self.panels.contains_key(id));
        if !is_centre_driven {
            return true; // viewport-only — chrome renders outside the dock
        }
        let in_dock: std::collections::HashSet<PanelId> = dock
            .iter_all_tabs()
            .filter_map(|(_, t)| {
                if let TabId::Singleton(id) = t {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect();
        side_browser
            .iter()
            .chain(side_browser_bottom.iter())
            .chain(right_inspector.iter())
            .chain(right_inspector_bottom.iter())
            .chain(bottom.iter())
            .chain(center.iter())
            .filter(|id| self.panels.contains_key(id))
            .all(|id| in_dock.contains(id))
    }
}

/// Bridges the reusable workspace-state capability to the concrete egui dock.
///
/// The provider keeps persisted session logic independent from this shell,
/// while all dock reconciliation remains owned by `WorkbenchLayout`.
pub struct WorkbenchLayoutStateProvider;

impl WorkspaceStateLayoutProvider for WorkbenchLayoutStateProvider {
    fn active_perspective(&self, world: &World) -> Option<String> {
        world
            .resource::<WorkbenchLayout>()
            .active_perspective()
            .map(|id| id.as_str().to_string())
    }

    fn active_tab_instance(&self, world: &World) -> Option<u64> {
        world.resource::<WorkbenchLayout>().active_tab_instance()
    }

    fn dock_layout_hash(&self, world: &World) -> u64 {
        world.resource::<WorkbenchLayout>().dock_layout_hash()
    }

    fn capture_perspective_docks(
        &self,
        world: &World,
    ) -> std::collections::HashMap<String, lunco_workbench_state::PerspectiveDockSnapshot> {
        world
            .resource::<WorkbenchLayout>()
            .capture_perspective_docks()
    }

    fn activate_perspective_by_str(&self, world: &mut World, id: &str) -> bool {
        world
            .resource_mut::<WorkbenchLayout>()
            .activate_perspective_by_str(id)
    }

    fn seed_perspective_docks(
        &self,
        world: &mut World,
        docks: &std::collections::HashMap<String, lunco_workbench_state::PerspectiveDockSnapshot>,
        id_map: &std::collections::HashMap<(&'static str, u64), u64>,
        discard_unmapped_kinds: &std::collections::HashSet<&'static str>,
    ) {
        world
            .resource_mut::<WorkbenchLayout>()
            .seed_perspective_docks(docks, id_map, discard_unmapped_kinds);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_dock::egui;
    use lunco_workbench_core::PanelCtx;

    struct TestInstancePanel(PanelId);

    impl InstancePanel for TestInstancePanel {
        fn kind(&self) -> PanelId {
            self.0
        }

        fn default_slot(&self) -> PanelSlot {
            PanelSlot::Center
        }

        fn title(&self, _world: &World, instance: u64) -> String {
            format!("{} #{instance}", self.0.as_str())
        }

        fn render(&mut self, _ui: &mut egui::Ui, _ctx: &mut PanelCtx, _instance: u64) {}
    }

    #[test]
    fn reconciliation_drops_stale_document_views_but_keeps_stable_instance_tabs() {
        let usd = PanelId("usd::preview_view");
        let graph = PanelId("modelica_graph");
        let mut layout = WorkbenchLayout::default();
        layout.register_instance_panel(TestInstancePanel(usd));
        layout.register_instance_panel(TestInstancePanel(graph));
        let saved = serde_json::to_value(DockState::new(vec![
            TabId::instance(usd, 17),
            TabId::instance(graph, 4),
        ]))
        .expect("dock serializes");
        let discarded = std::collections::HashSet::from([usd.0]);
        let remapped = HashMap::from([((usd.0, 17), 8)]);

        let restored = layout
            .reconcile_dock(saved, &remapped, &discarded)
            .expect("stable instance tab remains in the dock");
        let tabs: Vec<_> = restored.iter_all_tabs().map(|(_, tab)| *tab).collect();

        assert_eq!(
            tabs,
            vec![TabId::instance(usd, 8), TabId::instance(graph, 4)]
        );

        let stale_only = serde_json::to_value(DockState::new(vec![TabId::instance(usd, 18)]))
            .expect("stale dock serializes");
        assert!(
            layout
                .reconcile_dock(stale_only, &HashMap::new(), &discarded)
                .is_none()
        );
    }
}
