//! UI helpers — shared utilities for panel rendering.

use bevy_egui::egui;

/// Flip a [`egui::collapsing_header::CollapsingState`] open/closed and
/// persist the result.
///
/// `toggle` alone does not store, so this reloads, flips, and stores in
/// one step. Prefer [`collapsing_row`] for the common case; reach for
/// this only when you cannot express the row through that helper.
pub fn toggle_collapsing(ui: &egui::Ui, id: egui::Id, default_open: bool) {
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        id,
        default_open,
    );
    state.toggle(ui);
    state.store(ui.ctx());
}

/// A collapsible tree row whose **label click folds/unfolds** the row,
/// just like clicking the disclosure triangle.
///
/// egui's plain `CollapsingHeader` already folds on a header click, but
/// rows that need their label to carry a *custom* action (open a doc,
/// focus a prim in the viewport, rename on double-click, …) must use a
/// manual `CollapsingState` + a `Label::sense(click())` — which then
/// swallows the click and stops folding. This helper restores the
/// fold-on-label behaviour and keeps the fiddly state dance
/// (`load → show_header → body → toggle → store`) in one place.
///
/// `add_header` renders the row's header content and returns `true`
/// when the row should fold/unfold this frame (typically the label
/// `Response::clicked()`, minus whatever gesture the caller reserves
/// for its own action — e.g. double-click). `add_body` renders the
/// children shown while expanded.
pub fn collapsing_row(
    ui: &mut egui::Ui,
    id: egui::Id,
    default_open: bool,
    add_header: impl FnOnce(&mut egui::Ui) -> bool,
    add_body: impl FnOnce(&mut egui::Ui),
) {
    let state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        id,
        default_open,
    );
    let mut toggle = false;
    state
        .show_header(ui, |ui| toggle = add_header(ui))
        .body(add_body);
    if toggle {
        toggle_collapsing(ui, id, default_open);
    }
}
