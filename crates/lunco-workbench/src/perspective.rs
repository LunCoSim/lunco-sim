//! Perspectives — named layout presets the user switches between.
//!
//! A **Perspective** is a task-oriented chrome switch ("Build", "Simulate",
//! "Analyze", "Plan", "Observe") that rearranges the editor's dock so the
//! right panels are in the right slots for the current activity. The term
//! follows Eclipse / NetBeans; Blender uses the same idea under the name
//! "Workspace" but LunCoSim reserves that word for the editor session
//! (`lunco-workspace` crate).
//!
//! See `docs/architecture/11-workbench.md` § 4 for the original design.
//! v0.2 ships the mechanism (trait + registry + switcher UI); the standard
//! set of Perspectives is composed by the host app as it registers panels,
//! not hardcoded here.

use crate::{PanelId, UiIcon, WorkbenchLayout};

/// Stable identifier for a Perspective.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PerspectiveId(pub &'static str);

impl PerspectiveId {
    /// The raw string form.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// A named slot-assignment preset.
///
/// Panels are registered once and exist for the life of the app. A
/// Perspective decides **which panels occupy which slots** for this UX
/// mode and triggers a rebuild of the underlying `egui_dock` tree. Its slot
/// intent remains authoritative when panel renderers register later.
/// Switching Perspectives is non-destructive — no panel is torn down,
/// only the dock layout changes.
pub trait Perspective: Send + Sync + 'static {
    /// Stable ID used as a registry key and in the tab label.
    fn id(&self) -> PerspectiveId;

    /// Human-readable title for the Perspective tab.
    fn title(&self) -> String;

    /// Semantic icon painted by the workbench in the Perspective tab.
    ///
    /// Icons are part of the perspective contract rather than encoded in the
    /// title. This keeps navigation legible with every host font and gives
    /// the title bar one renderer for all registered perspectives.
    fn icon(&self) -> UiIcon;

    /// Whether this perspective is shown in the default title-bar switcher.
    ///
    /// A perspective can remain registered and activatable by a tutorial or
    /// [`crate::ActivatePerspective`] while staying out of the everyday
    /// navigation chrome. This is useful for authoring tools that are
    /// intentionally entered from a guided flow or an explicit command.
    fn show_in_switcher(&self) -> bool {
        true
    }

    /// Apply this Perspective's slot assignments to the layout.
    ///
    /// Implementations call the slot setters on `layout`; each setter
    /// updates the slot intent and triggers a dock rebuild.
    fn apply(&self, layout: &mut WorkbenchLayout);

    /// Whether this perspective may restore its cached dock. Presentation
    /// workspaces can opt out to guarantee that documents never cover the scene.
    fn restores_cached_layout(&self) -> bool {
        true
    }

    /// Whether the live 3D scene remains visible when this perspective's
    /// transient side or bottom panels are opened.
    ///
    /// A presentation perspective can use the full window as its viewport
    /// instead of mounting [`crate::viewport::ViewportPanel`]. Such a
    /// perspective must opt in here so opening a panel does not turn its
    /// full-window scene off. Perspectives whose central content owns the
    /// window remain hidden by default unless their layout contains the
    /// viewport panel.
    fn scene_visible_when_docked(&self) -> bool {
        false
    }

    /// Revision of the authored default layout.
    ///
    /// Increment this when a perspective's canonical slot arrangement changes.
    /// Persisted snapshots from an older revision are ignored once, allowing
    /// the new default to take effect without disabling user layout persistence
    /// after that first rebuild.
    fn layout_revision(&self) -> u32 {
        0
    }
}

// ─────────────────────────────────────────────────────────────────────
// Slot-assignment helpers callable from `Perspective::apply`.
// ─────────────────────────────────────────────────────────────────────

impl WorkbenchLayout {
    /// Whether the active perspective owns a full-window scene that should
    /// remain behind transient dock chrome even without a viewport panel tab.
    pub(crate) fn active_perspective_scene_visible_when_docked(&self) -> bool {
        self.active_perspective
            .and_then(|active| self.perspectives.iter().find(|p| p.id() == active))
            .is_some_and(|perspective| perspective.scene_visible_when_docked())
    }

    /// Dock a single panel in the side browser. Pass `None` to remove
    /// the side browser from the current workspace's preset.
    pub fn set_side_browser(&mut self, id: Option<PanelId>) {
        self.side_browser = id.into_iter().collect();
        self.side_browser_bottom.clear();
        self.rebuild_dock();
    }

    /// Dock multiple panels in the side browser as tabs (in order).
    pub fn set_side_browser_tabs(&mut self, ids: Vec<PanelId>) {
        self.side_browser = ids;
        self.side_browser_bottom.clear();
        self.rebuild_dock();
    }

    /// Dock a top and optional bottom panel group in the side browser.
    /// Panels within each group remain tabs; the two groups are stacked.
    pub fn set_side_browser_stacked(&mut self, top: Vec<PanelId>, bottom: Vec<PanelId>) {
        self.side_browser = top;
        self.side_browser_bottom = bottom;
        self.rebuild_dock();
    }

    /// Replace the Center-slot tab set with the given panels (in tab
    /// order). Active tab is clamped to the new length. Pass an empty
    /// list to leave the central region free for a 3D viewport.
    pub fn set_center(&mut self, ids: Vec<PanelId>) {
        if self.active_center_tab >= ids.len() {
            self.active_center_tab = ids.len().saturating_sub(1);
        }
        self.center = ids;
        self.rebuild_dock();
    }

    /// Append a panel to the Center tab strip if not already present.
    pub fn add_to_center(&mut self, id: PanelId) {
        if !self.center.contains(&id) {
            self.center.push(id);
            self.rebuild_dock();
        }
    }

    /// Select which Center tab is visible (by index). Out-of-range is a
    /// no-op. Note: under egui_dock, the user can also click tabs
    /// directly to switch.
    pub fn set_active_center_tab(&mut self, index: usize) {
        if index < self.center.len() {
            self.active_center_tab = index;
        }
    }

    /// Select which Center tab is visible by panel id. No-op if not
    /// registered as a Center tab.
    pub fn set_active_center_panel(&mut self, id: PanelId) {
        if let Some(pos) = self.center.iter().position(|p| *p == id) {
            self.active_center_tab = pos;
        }
    }

    /// Dock a single panel in the right inspector. `None` removes it.
    pub fn set_right_inspector(&mut self, id: Option<PanelId>) {
        self.right_inspector = id.into_iter().collect();
        self.right_inspector_bottom.clear();
        self.rebuild_dock();
    }

    /// Dock multiple panels in the right inspector as tabs (in order).
    pub fn set_right_inspector_tabs(&mut self, ids: Vec<PanelId>) {
        self.right_inspector = ids;
        self.right_inspector_bottom.clear();
        self.rebuild_dock();
    }

    /// Dock a top and optional bottom panel group in the right inspector.
    /// Panels within each group remain tabs; the two groups are stacked.
    pub fn set_right_inspector_stacked(&mut self, top: Vec<PanelId>, bottom: Vec<PanelId>) {
        self.right_inspector = top;
        self.right_inspector_bottom = bottom;
        self.rebuild_dock();
    }

    /// Dock a single panel in the bottom dock. `None` removes it.
    pub fn set_bottom(&mut self, id: Option<PanelId>) {
        self.bottom = id.into_iter().collect();
        self.rebuild_dock();
    }

    /// Dock multiple panels in the bottom dock as tabs (in order).
    pub fn set_bottom_tabs(&mut self, ids: Vec<PanelId>) {
        self.bottom = ids;
        self.rebuild_dock();
    }

    /// Show or hide the activity bar on the far left.
    pub fn set_activity_bar(&mut self, visible: bool) {
        self.activity_bar = visible;
    }
}
