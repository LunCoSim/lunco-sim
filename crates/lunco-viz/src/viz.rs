//! Visualization trait + config.
//!
//! A [`Visualization`] is the data-to-geometry transform. It declares
//! which signals it consumes (via typed roles), which view targets it
//! can render into, and the actual rendering logic.
//!
//! A [`VisualizationConfig`] is the serializable description of *one*
//! bound viz instance — "plot `thrust` on the main time-series" is a
//! config with kind `line_plot`, view `Panel2D(…)`, and a list of
//! `SignalBinding`s.
//!
//! The config and the impl are intentionally separate: the config can
//! be persisted to a workspace file, round-tripped, shared, and edited
//! in an inspector UI, all without needing the concrete renderer.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::signal::{SignalRef, SignalType};
use crate::view::{Panel2DCtx, ViewKind, ViewTarget};

/// Identifier for a kind of visualization.
///
/// Backed by `Cow<'static, str>` so built-in kinds can be declared as
/// compile-time constants (`VizKindId::new_static("line_plot")`) while
/// workspace-loaded kinds can carry owned `String`s. Third-party
/// crates register their own viz kinds by picking a unique identifier
/// string; no enum changes needed.
///
/// Convention: lowercase, dotted, domain.name — e.g. `line_plot`,
/// `modelica.icon_block`, `avian.contact_forces`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VizKindId(pub std::borrow::Cow<'static, str>);

impl VizKindId {
    /// Const-legal constructor for built-in kind ids.
    pub const fn new_static(s: &'static str) -> Self {
        Self(std::borrow::Cow::Borrowed(s))
    }
    /// Runtime constructor — typically only used by workspace loaders.
    pub fn new(s: impl Into<String>) -> Self {
        Self(std::borrow::Cow::Owned(s.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Unique identifier for one live visualization instance.
///
/// Generated via [`VizId::next`](Self::next); monotone across a
/// session. Round-trips through workspace files as a `u64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct VizId(pub u64);

impl VizId {
    /// Allocate the next `VizId`. Uses a process-global atomic counter;
    /// collisions with ids loaded from disk are possible if a workspace
    /// was created in another session — we'll revisit when persistence
    /// lands.
    pub fn next() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        VizId(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    pub fn raw(self) -> u64 {
        self.0
    }
}

/// One signal bound to one role of a viz. "Role" is viz-kind-specific
/// (a `LinePlot` has a `"y"` role; an `Arrow3D` has `"position"` and
/// `"vector"`); each viz kind's [`Visualization::role_schema`] declares
/// the expected set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalBinding {
    pub source: SignalRef,
    pub role: String,
    /// Legend label override. Defaults to `source.path` when `None`.
    #[serde(default)]
    pub label: Option<String>,
    /// User-chosen line/marker color, if any. `None` means
    /// "auto-assign from the palette".
    #[serde(default, with = "color_opt")]
    pub color: Option<bevy_egui::egui::Color32>,
    /// Whether the binding currently contributes to the render.
    /// Click-to-toggle on the legend flips this without deleting the
    /// binding.
    #[serde(default = "default_true")]
    pub visible: bool,
}

fn default_true() -> bool { true }

// Small serde glue for `Option<Color32>` — keeps workspace files
// human-readable (`[r, g, b]` / `null`).
mod color_opt {
    use bevy_egui::egui::Color32;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(c: &Option<Color32>, s: S) -> Result<S::Ok, S::Error> {
        c.map(|c| [c.r(), c.g(), c.b()]).serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Color32>, D::Error> {
        let opt: Option<[u8; 3]> = Option::deserialize(d)?;
        Ok(opt.map(|[r, g, b]| Color32::from_rgb(r, g, b)))
    }
}

/// Per-role declaration of what a viz expects. Used by the inspector
/// UI to filter signal pick-lists and validate bindings.
#[derive(Debug, Clone)]
pub struct RoleSpec {
    pub role: &'static str,
    pub accepted_types: &'static [SignalType],
    /// If true, a single binding fulfils the role; if false, the viz
    /// accepts many bindings (e.g. a line plot's `"y"` role takes
    /// arbitrarily many signals, each rendered as its own line).
    pub single: bool,
}

/// Serializable description of one active visualization.
///
/// Everything needed to rebuild a plot from a saved workspace lives
/// here. No references to in-memory data: `inputs` carry `SignalRef`s
/// which the [`SignalRegistry`](crate::signal::SignalRegistry) resolves
/// on each render pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisualizationConfig {
    pub id: VizId,
    pub title: String,
    pub kind: VizKindId,
    pub view: ViewTarget,
    #[serde(default)]
    pub inputs: Vec<SignalBinding>,
    /// Kind-specific style / axis / annotation blob. Each viz kind
    /// defines its own schema; we serialize as opaque JSON so the core
    /// doesn't need to know about every kind's options.
    #[serde(default)]
    pub style: serde_json::Value,
}

/// The contract a viz kind implements.
///
/// Only `render_panel_2d` is required in v0.1. 3D rendering paths will
/// land as additional (default no-op) trait methods once the
/// `Viewport3D` / `Panel3D` views are wired.
pub trait Visualization: Send + Sync + 'static {
    /// Identifier for this kind. Must be unique across the process.
    fn kind_id(&self) -> VizKindId;

    /// Display name for the inspector "new viz" picker.
    fn display_name(&self) -> &'static str;

    /// What signal roles this viz accepts.
    fn role_schema(&self) -> &'static [RoleSpec];

    /// Which view kinds this viz can render into.
    fn compatible_views(&self) -> &'static [ViewKind];

    /// Render into a 2D panel. Default: no-op. Implement if your viz
    /// declares `ViewKind::Panel2D` compatibility.
    fn render_panel_2d(&self, _ctx: &mut Panel2DCtx, _config: &VisualizationConfig) {}

    // Future:
    // fn render_viewport_3d(&self, ctx: &mut Viewport3DCtx, config: &...) {}
    // fn render_panel_3d(&self, ctx: &mut Panel3DCtx, config: &...) {}
    // fn inspector_ui(&self, ui: &mut egui::Ui, config: &mut VisualizationConfig) {}
}
