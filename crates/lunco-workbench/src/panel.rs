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
    fn title(&self) -> String;

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
