//! The `Panel` trait and companion types.

use bevy::prelude::*;
use bevy_egui::egui;

/// Stable identifier for a panel.
///
/// Today a static string; later may grow to include versioning or a
/// dock-tree address. Keeping it a newtype lets us evolve without
/// breaking callers who use it as a dictionary key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PanelId(pub &'static str);

impl PanelId {
    /// The raw string form, for debug output and serialization.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// Which region of the workbench a panel lives in.
///
/// Maps to the layout regions in `docs/architecture/11-workbench.md` § 3.
/// A single slot holds one panel in v0.1; tabbing and splitting come later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PanelSlot {
    /// Left side browser, slides out from the activity bar.
    /// Typical: Scene Tree, Library Browser, Mission Outline.
    SideBrowser,
    /// Central tabbed region — where the primary content lives in apps
    /// without a 3D viewport (e.g. `modelica_workbench` shows Code /
    /// Diagram / Documentation as central tabs). 3D apps leave this
    /// empty so their world renders through.
    ///
    /// Multiple panels can share Center; they appear as tabs at the top
    /// of the central region. Exactly one is visible at a time.
    Center,
    /// Right-side context-aware inspector.
    /// Typical: Properties, Modelica Inspector, Attribute Editor.
    RightInspector,
    /// Bottom dock, toggleable.
    /// Typical: Console, Plots, Timeline, Spawn Palette.
    Bottom,
    /// Detached into its own OS window.
    /// Not rendered by v0.1 — placeholder for the multi-viewport story.
    Floating,
}

/// A dockable unit of UI rendered by [`crate::WorkbenchPlugin`].
///
/// Panels take `&mut World` because they routinely need to read and
/// write multiple resources (a Document registry, selection state,
/// worker channels, …). Keeping the signature uniform avoids the
/// `ui` / `ui_world` split we inherited from `bevy_workbench`, which
/// forced every nontrivial panel into the `ui_world` branch anyway.
pub trait Panel: Send + Sync + 'static {
    /// Stable ID for this panel (used as a layout key).
    fn id(&self) -> PanelId;

    /// Human-readable title rendered in the tab / header bar.
    ///
    /// This is the static fallback. Override [`dynamic_title`](Self::dynamic_title)
    /// when the tab label should reflect live content (e.g. the currently
    /// open file).
    fn title(&self) -> String;

    /// Title used by the dock, called once per frame with world access.
    ///
    /// Defaults to [`title`](Self::title). Override to show live state —
    /// e.g. a Model-view tab returning the open file's name instead of a
    /// static label. Panels that don't override pay no overhead beyond
    /// a virtual dispatch.
    fn dynamic_title(&self, _world: &World) -> String {
        self.title()
    }

    /// Where to dock this panel by default when registered.
    fn default_slot(&self) -> PanelSlot;

    /// Whether the user can close the panel. Closable panels get an `×`.
    fn closable(&self) -> bool {
        true
    }

    /// Whether the dock should leave the panel's tab body transparent
    /// instead of filling it with the theme background colour.
    ///
    /// Default `false` (opaque) — what every normal panel wants. The
    /// viewport panel returns `true` so Bevy's 3D scene, which renders
    /// behind egui, shows through the rect.
    fn transparent_background(&self) -> bool {
        false
    }

    /// Render the panel contents. Panels own their reads and writes to
    /// the world; the workbench shell only provides the `&mut egui::Ui`.
    fn render(&mut self, ui: &mut egui::Ui, world: &mut World);
}

// ─────────────────────────────────────────────────────────────────────
// Multi-instance tabs
// ─────────────────────────────────────────────────────────────────────

/// A panel *kind* that can exist as multiple tabs at once, each backed
/// by a distinct `instance: u64` id.
///
/// Use this for "one tab per open document" workflows — a Modelica
/// model view, a USD scene view, a script editor. The `instance` id is
/// opaque to the workbench (typically a `DocumentId`'s raw `u64`); the
/// host domain decides what it means. The workbench just dispatches
/// render/title/close to the right `InstancePanel` based on the tab's
/// registered `kind`.
///
/// Singleton panels (Package Browser, Telemetry, Graphs, …) keep using
/// [`Panel`] — that trait's semantics are unchanged.
pub trait InstancePanel: Send + Sync + 'static {
    /// The tab-kind id. All tabs of this kind share one
    /// `InstancePanel` instance; only the `instance: u64` differs.
    fn kind(&self) -> PanelId;

    /// Default dock slot for newly-opened tabs of this kind.
    fn default_slot(&self) -> PanelSlot;

    /// Title shown in the tab header for `instance`.
    ///
    /// Runs each frame with world access so titles can follow live
    /// state (e.g. the open document's display name).
    fn title(&self, world: &World, instance: u64) -> String;

    /// Whether tabs of this kind are closable by the user.
    fn closable(&self) -> bool {
        true
    }

    /// Whether the tab body should be rendered with a transparent
    /// background (defers to dock theme otherwise).
    fn transparent_background(&self) -> bool {
        false
    }

    /// Render one tab instance.
    fn render(&mut self, ui: &mut egui::Ui, world: &mut World, instance: u64);
}

/// Identity of a tab in the dock.
///
/// - `Singleton(id)` — the classic one-panel-per-id tab, backed by a
///   [`Panel`] impl.
/// - `Instance { kind, instance }` — one of many tabs of the same
///   kind, dispatched to the matching [`InstancePanel`] with the
///   given `instance` discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TabId {
    /// A singleton panel tab (legacy one-per-id).
    Singleton(PanelId),
    /// A multi-instance tab. `kind` selects the renderer; `instance`
    /// is the per-tab discriminant (usually a raw `DocumentId`).
    Instance {
        /// The [`InstancePanel`] kind that renders this tab.
        kind: PanelId,
        /// The tab's instance id, interpreted by the registered kind.
        instance: u64,
    },
}

impl TabId {
    /// Shorthand for a singleton tab id.
    pub const fn singleton(id: PanelId) -> Self {
        TabId::Singleton(id)
    }

    /// Shorthand for an instance tab id.
    pub const fn instance(kind: PanelId, instance: u64) -> Self {
        TabId::Instance { kind, instance }
    }

    /// Raw identity string — stable across calls, used as the
    /// `egui::Id` seed for per-tab persistent widget state.
    pub fn debug_id(&self) -> String {
        match self {
            TabId::Singleton(id) => format!("s:{}", id.as_str()),
            TabId::Instance { kind, instance } => {
                format!("i:{}:{}", kind.as_str(), instance)
            }
        }
    }
}

impl From<PanelId> for TabId {
    fn from(id: PanelId) -> Self {
        TabId::Singleton(id)
    }
}
