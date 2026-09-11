//! Renderer-independent perspective contracts.

use crate::{PanelId, PanelSlot};

/// Stable perspective identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PerspectiveId(pub &'static str);

impl PerspectiveId {
    /// Return the stable string representation.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// One slot's panel declarations in a perspective plan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PerspectiveSlotPlan {
    /// Primary panels in display order.
    pub primary: Vec<PanelId>,
    /// Optional stacked lower panels.
    pub secondary: Vec<PanelId>,
}

impl PerspectiveSlotPlan {
    /// Create an empty slot plan.
    pub const fn new() -> Self {
        Self {
            primary: Vec::new(),
            secondary: Vec::new(),
        }
    }

    /// Set a single panel or clear the slot.
    pub fn single(mut self, panel: Option<PanelId>) -> Self {
        self.primary = panel.into_iter().collect();
        self.secondary.clear();
        self
    }

    /// Set a tab group.
    pub fn tabs(mut self, panels: impl IntoIterator<Item = PanelId>) -> Self {
        self.primary = panels.into_iter().collect();
        self.secondary.clear();
        self
    }

    /// Set two stacked groups.
    pub fn stacked(
        mut self,
        primary: impl IntoIterator<Item = PanelId>,
        secondary: impl IntoIterator<Item = PanelId>,
    ) -> Self {
        self.primary = primary.into_iter().collect();
        self.secondary = secondary.into_iter().collect();
        self
    }
}

/// One instance tab requested by a perspective's initial layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerspectiveInstanceTab {
    /// Registered instance-panel kind.
    pub kind: PanelId,
    /// Domain-owned instance identity.
    pub instance: u64,
    /// Semantic slot in which the shell should place it.
    pub slot: PanelSlot,
}

/// Renderer-independent result of applying a perspective.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PerspectiveLayoutPlan {
    /// Whether the activity bar is visible.
    pub activity_bar: bool,
    /// Side-browser slot declaration.
    pub side_browser: PerspectiveSlotPlan,
    /// Center slot declaration.
    pub center: PerspectiveSlotPlan,
    /// Right-inspector slot declaration.
    pub right_inspector: PerspectiveSlotPlan,
    /// Bottom slot declaration.
    pub bottom: PerspectiveSlotPlan,
    /// Initial instance tabs to open after singleton layout construction.
    pub instance_tabs: Vec<PerspectiveInstanceTab>,
    /// Initially selected center tab, if any.
    pub active_center_tab: Option<usize>,
}

impl PerspectiveLayoutPlan {
    /// Create an empty plan with the shell's default activity-bar state.
    pub const fn new() -> Self {
        Self {
            activity_bar: false,
            side_browser: PerspectiveSlotPlan::new(),
            center: PerspectiveSlotPlan::new(),
            right_inspector: PerspectiveSlotPlan::new(),
            bottom: PerspectiveSlotPlan::new(),
            instance_tabs: Vec::new(),
            active_center_tab: None,
        }
    }

    /// Add an initial instance tab.
    pub fn open_instance(mut self, kind: PanelId, instance: u64, slot: PanelSlot) -> Self {
        self.instance_tabs.push(PerspectiveInstanceTab {
            kind,
            instance,
            slot,
        });
        self
    }
}

/// Named task-oriented shell layout.
pub trait Perspective: Send + Sync + 'static {
    /// Stable id.
    fn id(&self) -> PerspectiveId;
    /// Human-readable title.
    fn title(&self) -> String;
    /// Whether the shell shows this perspective in its normal switcher.
    fn show_in_switcher(&self) -> bool {
        true
    }
    /// Return the semantic layout plan. The concrete shell owns dock materialization.
    fn layout(&self) -> PerspectiveLayoutPlan;
    /// Whether a cached concrete layout may be restored.
    fn restores_cached_layout(&self) -> bool {
        true
    }
    /// Whether the primary scene remains visible behind transient dock chrome.
    fn scene_visible_when_docked(&self) -> bool {
        false
    }
    /// Which subsystem owns a plain primary scene click in this perspective.
    fn scene_interaction_mode(&self) -> lunco_core::SceneInteractionMode {
        lunco_core::SceneInteractionMode::Simulation
    }
    /// Revision of the authored default layout.
    fn layout_revision(&self) -> u32 {
        0
    }
}
