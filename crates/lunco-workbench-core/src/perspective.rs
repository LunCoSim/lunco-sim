//! Renderer-independent perspective contracts.

use crate::{PanelId, PanelSlot, TabId};
use std::sync::Arc;

/// Stable perspective identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PerspectiveId(pub Arc<str>);

impl PerspectiveId {
    /// Return the stable string representation.
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }

    /// Own a stable identity without leaking dynamically authored names.
    pub fn new(id: impl Into<Arc<str>>) -> Self {
        Self(id.into())
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

/// Viewport corner for a presentation panel. Placement is authored intent;
/// the concrete shell clamps it to the available viewport on every resize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// A registered panel hosted over the scene without a dock tab or title bar.
/// Dimensions and inset are logical pixels, independent of capture resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerspectiveFloatingPanel {
    pub tab: TabId,
    pub corner: PanelCorner,
    pub width: u32,
    pub height: u32,
    pub inset: u32,
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
    /// Borderless presentation hosts, exclusive with dock placement.
    pub floating_panels: Vec<PerspectiveFloatingPanel>,
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
            floating_panels: Vec::new(),
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
    /// Task guidance displayed by the shell and perspective help.
    fn description(&self) -> &str {
        ""
    }
    /// Whether the title bar pins this perspective; View → Perspectives lists every registered task.
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
    fn scene_interaction_mode(&self) -> lunco_interaction_core::SceneInteractionMode {
        lunco_interaction_core::SceneInteractionMode::Simulation
    }
    /// Revision of the authored default layout.
    fn layout_revision(&self) -> u32 {
        0
    }
}

/// Admission state consumed by session restoration before applying saved docks.
#[derive(bevy::prelude::Resource, Clone, Debug, PartialEq, Eq)]
pub struct PerspectiveAdmission {
    /// Exact Twin for which the current policy result is being prepared.
    pub twin_id: Option<u64>,
    /// Work is outstanding; restoring a dock must wait for its definitions.
    pub preparing: bool,
}
impl Default for PerspectiveAdmission {
    fn default() -> Self {
        Self {
            twin_id: None,
            preparing: true,
        }
    }
}

/// Explicit navigation takes precedence over asynchronous session restoration.
#[derive(bevy::prelude::Resource, Clone, Debug, Default)]
pub struct ExplicitPerspectiveChoice {
    /// Exact Twin lease, or the application with no Twin.
    pub twin_id: Option<u64>,
    /// Last deliberately selected registered task.
    pub id: Option<String>,
}
