//! Data structures for Modelica model views.

use bevy::prelude::*;
use lunco_doc::DocumentId;
use lunco_workbench::PanelId;

/// The `PanelId` under which `ModelViewPanel` is registered.
pub const MODEL_VIEW_KIND: PanelId = PanelId("modelica_model_view");

/// Which rendering mode a model tab is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModelViewMode {
    /// Raw Modelica source (egui TextEdit).
    Text,
    /// Block-diagram canvas, rendered on `lunco-canvas`.
    #[default]
    Canvas,
    /// The class's own `Icon` annotation rendering.
    Icon,
    /// The class's `Documentation` annotation rendered as text.
    Docs,
}

/// Per-tab state for a [`ModelViewPanel`] instance.
#[derive(Debug, Clone)]
pub struct ModelTabState {
    pub doc: DocumentId,
    pub drilled_class: Option<String>,
    pub view_mode: ModelViewMode,
    pub pinned: bool,
}

pub type TabId = u64;

#[derive(Resource, Default, Debug, Clone)]
pub struct TabRenderContext {
    pub tab_id: Option<TabId>,
    pub doc: Option<DocumentId>,
    pub drilled_class: Option<String>,
}

impl TabRenderContext {
    pub fn current(&self) -> Option<(DocumentId, Option<&str>)> {
        self.doc.map(|d| (d, self.drilled_class.as_deref()))
    }
}
