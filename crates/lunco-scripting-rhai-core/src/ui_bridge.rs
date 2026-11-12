//! Typed script-to-UI requests.
//!
//! The scripting layer owns the neutral event contract; an editor or another
//! host may choose how to render it. No domain names or JSON payloads belong in
//! this bridge.

use bevy::prelude::Event;
use lunco_telemetry_core::TelemetryValue;

/// Exact scene-catalog facts assembled by the application asset/workspace owners.
/// This bridge carries no menu structure or presentation policy.
#[derive(Event, Clone, Debug, Default, PartialEq, Eq)]
pub struct ApplicationSceneCatalogChanged {
    /// Qualified scene address accepted by `LoadScene`, if one is committed.
    pub current_scene: Option<String>,
    /// The active workspace Twin, with a stable source identity.
    pub active_twin: Option<SceneCatalogTwin>,
    /// Open workspace Twins, including those with no scene inventory entries.
    pub twins: Vec<SceneCatalogTwin>,
    /// Verified cached scenario revisions available to open offline.
    pub cached_twins: Vec<CachedScenarioTwin>,
    /// Whether the application includes scenario synchronization.
    pub download_sync_enabled: bool,
    /// The saved test-asset visibility preference. Rhai chooses its presentation.
    pub show_test_assets: bool,
    /// `false` means the engine inventory has not arrived yet.
    pub catalog_ready: bool,
    /// A registry or discovery failure. The Rhai policy presents this state explicitly.
    pub catalog_error: Option<String>,
    /// Scenes admitted by the engine inventory and open Twin manifests.
    pub scenes: Vec<SceneCatalogEntry>,
}

/// Source-owned identity and name facts for one open Twin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneCatalogTwin {
    pub id: u64,
    pub source_identity: String,
    pub source_ready: bool,
    /// Authored Twin manifest name, when present.
    pub name: Option<String>,
    /// Filesystem directory name, when it can be represented as text.
    pub directory: Option<String>,
    pub default_scene: Option<String>,
}

/// One loadable scene from the admitted engine or Twin inventory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneCatalogEntry {
    /// Fully qualified `lunco://` or `twin://` address.
    pub uri: String,
    /// Path relative to its owning engine or Twin inventory.
    pub relative_path: String,
    /// Source filename stem, without a presentation label applied.
    pub stem: String,
    /// Standard USD documentation description, when it has been read.
    pub description: Option<String>,
    /// Test classification from the asset catalog owner.
    pub is_test: bool,
    /// Twin identity when this scene belongs to an open workspace Twin.
    pub twin: Option<SceneCatalogTwin>,
}

/// One exact downloaded scenario revision admitted by the synchronization cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedScenarioTwin {
    pub scenario_id: String,
    pub revision: String,
    pub name: String,
    pub default_scene: Option<String>,
    pub total_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct ScriptMenuItem {
    pub label: String,
    pub tool: String,
    pub hook: String,
    pub args: TelemetryValue,
}

/// A script-authored top-level workbench menu contribution.
#[derive(Clone, Debug)]
pub struct ScriptWorkbenchMenu {
    pub label: String,
    /// Policy-defined position among contributions sharing this top-level label.
    pub order: i64,
    /// Draw a divider before this contribution when an earlier one is present.
    pub separator_before: bool,
    pub items: Vec<ScriptWorkbenchMenuItem>,
}

/// One leaf action or submenu in a script-authored workbench menu.
#[derive(Clone, Debug)]
pub struct ScriptWorkbenchMenuItem {
    pub label: String,
    pub tooltip: Option<String>,
    pub enabled: bool,
    /// Draw a divider without adding a labeled row.
    pub separator: bool,
    /// Generic radio or checkbox presentation supplied by the policy.
    pub selection: Option<ScriptWorkbenchSelection>,
    pub action: Option<ScriptWorkbenchMenuAction>,
    pub children: Vec<ScriptWorkbenchMenuItem>,
}

/// Selection semantics for an authored menu leaf.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptWorkbenchSelection {
    Radio(bool),
    Checkbox(bool),
}

/// A workbench menu leaf routed through the existing generic Rhai tool hook.
#[derive(Clone, Debug)]
pub struct ScriptWorkbenchMenuAction {
    pub tool: String,
    pub hook: String,
    pub args: TelemetryValue,
}

/// A quick-open destination composed by application policy.
#[derive(Clone, Debug)]
pub struct ScriptWorkbenchNavigationEntry {
    pub id: String,
    pub label: String,
    pub detail: String,
    pub category: String,
    pub action: ScriptWorkbenchMenuAction,
}

/// A script-authored action for the workbench's empty-viewport surface.
#[derive(Clone, Debug)]
pub struct ScriptViewportAction {
    pub label: String,
    pub description: String,
    pub enabled: bool,
    pub primary: bool,
    /// Generic visibility condition evaluated against the active workspace.
    pub visibility: ScriptViewportVisibility,
    pub action: ScriptWorkbenchMenuAction,
}

/// Workspace condition for a script-authored empty-viewport action.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScriptViewportVisibility {
    /// Show the action whenever its provider is applicable.
    #[default]
    Always,
    /// Show the action only when no Twin is active.
    NoActiveTwin,
}

#[derive(Event, Clone, Debug)]
pub enum ScriptUiRequest {
    ContextMenu {
        screen_position: [f32; 2],
        items: Vec<ScriptMenuItem>,
    },
    /// Replace one provider's complete workbench menu contribution.
    WorkbenchMenus {
        /// Stable owner key; updates replace this provider.
        provider: String,
        /// Workspace Twin identity, or `None` for application assets.
        twin_id: Option<u64>,
        menus: Vec<ScriptWorkbenchMenu>,
        /// Replaceable Settings submenu contributions authored by the same policy.
        settings_submenus: Vec<ScriptWorkbenchMenu>,
        /// Replaceable generic actions for the empty viewport.
        viewport_actions: Vec<ScriptViewportAction>,
        /// Optional policy-authored guidance for the empty viewport.
        viewport_guidance: Option<String>,
        /// Replaceable quick-open destinations contributed by the same policy.
        navigation_entries: Vec<ScriptWorkbenchNavigationEntry>,
    },
}
