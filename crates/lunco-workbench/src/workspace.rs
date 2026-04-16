//! Workspaces — named layout presets the user switches between.
//!
//! See `docs/architecture/11-workbench.md` § 4 for the design. v0.1 ships
//! the mechanism (trait + registry + switcher UI); the five standard
//! workspaces (Build / Simulate / Analyze / Plan / Observe) are composed
//! by the host app as it registers panels, not hardcoded here.

use crate::{PanelId, WorkbenchLayout};

/// Stable identifier for a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorkspaceId(pub &'static str);

impl WorkspaceId {
    /// The raw string form.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// A named slot-assignment preset.
///
/// Panels are registered once and exist for the life of the app. A
/// workspace decides **which panels occupy which slots, and whether
/// the bottom dock is open**, for this UX mode. Switching workspaces
/// is non-destructive — no panel is torn down — only slot assignments
/// change.
pub trait Workspace: Send + Sync + 'static {
    /// Stable ID used as a registry key and in the tab label.
    fn id(&self) -> WorkspaceId;

    /// Human-readable title for the workspace tab.
    fn title(&self) -> String;

    /// Apply this workspace's slot assignments to the layout.
    ///
    /// Implementations typically read panel ids from their own config
    /// and call [`SlotAssignments`] helpers on the layout.
    fn apply(&self, layout: &mut WorkbenchLayout);
}

/// Slot-assignment helpers callable from `Workspace::apply`.
///
/// Kept as methods on `WorkbenchLayout` so `apply` implementations
/// have one `&mut layout` parameter and a clear surface.
impl WorkbenchLayout {
    /// Dock a specific panel in the side browser. Overrides any prior
    /// occupant. Pass `None` to hide the side browser.
    pub fn set_side_browser(&mut self, id: Option<PanelId>) {
        self.side_browser = id;
    }

    /// Replace the Center-slot tab set with the given panels (in tab
    /// order). Active tab is clamped to the new length. Pass an empty
    /// list to leave the central region free for a 3D viewport.
    pub fn set_center(&mut self, ids: Vec<PanelId>) {
        if self.active_center_tab >= ids.len() {
            self.active_center_tab = ids.len().saturating_sub(1);
        }
        self.center = ids;
    }

    /// Append a panel to the Center tab strip if not already present.
    pub fn add_to_center(&mut self, id: PanelId) {
        if !self.center.contains(&id) {
            self.center.push(id);
        }
    }

    /// Select which Center tab is visible (by index). Out-of-range is a
    /// no-op.
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

    /// Dock a specific panel in the right inspector. `None` hides it.
    pub fn set_right_inspector(&mut self, id: Option<PanelId>) {
        self.right_inspector = id;
    }

    /// Dock a specific panel in the bottom dock. `None` clears + hides.
    pub fn set_bottom(&mut self, id: Option<PanelId>) {
        self.bottom = id;
        if id.is_none() {
            self.bottom_visible = false;
        } else {
            self.bottom_visible = true;
        }
    }

    /// Explicitly set bottom-dock visibility without changing the
    /// occupant. Useful for workspaces like Simulate that keep the
    /// same bottom content as Build but start it collapsed.
    pub fn set_bottom_visible(&mut self, visible: bool) {
        self.bottom_visible = visible;
    }

    /// Show or hide the activity bar on the far left.
    pub fn set_activity_bar(&mut self, visible: bool) {
        self.activity_bar = visible;
    }
}
