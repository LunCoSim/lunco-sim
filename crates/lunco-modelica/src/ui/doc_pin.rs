//! Per-panel "pin to model" overrides for singleton inspector panels.
//!
//! Telemetry and Inspector follow the active document tab by
//! default. With several model tabs open the user may want one panel
//! to stay on a specific model while they edit another — same
//! mechanic Dymola exposes as "pin to current model window". The
//! pin is a `None | Some(DocumentId)` per panel kind: `None` =
//! follow active, `Some(d)` = stay on `d` until cleared.
//!
//! Resolution is always `pin.or(active_document)`, so a pinned doc
//! that gets closed silently falls back to the active doc on the
//! next frame (and the close cleanup observer wipes the stale id).

use bevy::prelude::*;
use lunco_doc::DocumentId;
use lunco_experiments::TwinId;

#[derive(Resource, Default, Debug, Clone)]
pub struct DocPinState {
    /// Telemetry panel pin — locks the runtime telemetry view
    /// (simulator entity, inputs, signals) to a specific doc.
    pub telemetry: Option<DocumentId>,
    /// Inspector panel pin — locks the per-selection editor to a
    /// specific doc's canvas selection.
    pub inspector: Option<DocumentId>,
    /// Experiments panel pin — locks the experiment list + plot
    /// readouts to a specific doc's run history.
    pub experiments: Option<DocumentId>,
}

impl DocPinState {
    pub fn forget(&mut self, doc: DocumentId) {
        if self.telemetry == Some(doc) {
            self.telemetry = None;
        }
        if self.inspector == Some(doc) {
            self.inspector = None;
        }
        if self.experiments == Some(doc) {
            self.experiments = None;
        }
    }
}

/// Stable per-document scoping key for [`lunco_experiments::ExperimentRegistry`].
/// Each open document owns its own list of runs — opening Model A
/// and Model B side-by-side gives two independent experiment streams
/// instead of one mixed pile (different variable namespaces, no
/// useful cross-comparison).
pub fn twin_id_for_doc(doc: DocumentId) -> TwinId {
    TwinId(format!("doc:{}", doc.raw()))
}

/// Active document from the workspace (most-recently-focused tab).
pub fn active_doc(world: &World) -> Option<DocumentId> {
    world
        .get_resource::<lunco_workbench::WorkspaceResource>()?
        .active_document
}

/// `pin.telemetry.or(active_doc)`. Telemetry panel uses this to
/// decide which doc's simulator entity to bind to.
pub fn resolved_telemetry_doc(world: &World) -> Option<DocumentId> {
    let pin = world.get_resource::<DocPinState>().and_then(|s| s.telemetry);
    pin.or_else(|| active_doc(world))
}

/// `pin.inspector.or(active_doc)`. Inspector panel uses this to
/// decide which doc's canvas selection to inspect.
pub fn resolved_inspector_doc(world: &World) -> Option<DocumentId> {
    let pin = world.get_resource::<DocPinState>().and_then(|s| s.inspector);
    pin.or_else(|| active_doc(world))
}

/// `pin.experiments.or(active_doc)`. Experiments panel + plot
/// readouts use this to choose which doc's run history to show.
pub fn resolved_experiments_doc(world: &World) -> Option<DocumentId> {
    let pin = world
        .get_resource::<DocPinState>()
        .and_then(|s| s.experiments);
    pin.or_else(|| active_doc(world))
}

/// Which slot of [`DocPinState`] a header widget toggles.
#[derive(Copy, Clone, Debug)]
pub enum PinKind {
    Telemetry,
    Inspector,
    Experiments,
}

/// Render a one-line "follow active tab | 📌 pinned to {name}" row
/// at the top of an inspector panel. Click the pin button to lock
/// the panel onto the currently-active doc; click again to release.
pub fn render_pin_header(
    ui: &mut bevy_egui::egui::Ui,
    world: &mut World,
    kind: PinKind,
) {
    use bevy_egui::egui;

    let current_pin = world
        .get_resource::<DocPinState>()
        .map(|s| match kind {
            PinKind::Telemetry => s.telemetry,
            PinKind::Inspector => s.inspector,
            PinKind::Experiments => s.experiments,
        })
        .unwrap_or(None);
    let active = active_doc(world);
    let target = current_pin.or(active);
    let muted = world
        .get_resource::<lunco_theme::Theme>()
        .map(|t| t.tokens.text_subdued)
        .unwrap_or(egui::Color32::from_rgb(140, 140, 160));

    let label = match target {
        Some(doc) => doc_display_name(world, doc),
        None => "no document".to_string(),
    };
    let (icon, hover) = match current_pin {
        Some(_) => (
            "📌",
            "Pinned to this model. Click to release and follow the active tab.",
        ),
        None => (
            "📍",
            "Following the active tab. Click to pin to the current model.",
        ),
    };
    // Visual accent when pinned — the 📍 / 📌 distinction alone is
    // too subtle in some fonts. A coloured PIN badge reads at a
    // glance and matches Dymola's "Current model" indicator.
    let accent = world
        .get_resource::<lunco_theme::Theme>()
        .map(|t| t.tokens.accent)
        .unwrap_or(egui::Color32::from_rgb(110, 170, 230));

    let mut toggle = false;
    ui.horizontal(|ui| {
        if ui.small_button(icon).on_hover_text(hover).clicked() {
            toggle = true;
        }
        let mut text = egui::RichText::new(label).small();
        if current_pin.is_some() {
            text = text.color(accent).strong();
            ui.label(
                egui::RichText::new("PIN")
                    .color(accent)
                    .strong()
                    .small(),
            );
        } else {
            text = text.color(muted);
        }
        ui.label(text);
    });
    if toggle {
        if let Some(mut state) = world.get_resource_mut::<DocPinState>() {
            let slot = match kind {
                PinKind::Telemetry => &mut state.telemetry,
                PinKind::Inspector => &mut state.inspector,
                PinKind::Experiments => &mut state.experiments,
            };
            *slot = match *slot {
                Some(_) => None,
                None => active,
            };
        }
    }
}

pub fn doc_display_name(world: &World, doc: DocumentId) -> String {
    world
        .get_resource::<crate::state::ModelicaDocumentRegistry>()
        .and_then(|reg| reg.host(doc))
        .map(|host| host.document().origin().display_name())
        .unwrap_or_else(|| format!("doc#{:?}", doc))
}
