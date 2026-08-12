//! The solver combo box, in one place.
//!
//! Two surfaces let a user pin a solver — the experiments Setup dialog and the
//! FastRun setup — and both edit the same `RunBounds::solver`. Rendering the
//! widget twice means two lists to keep in step with the registry and two hover
//! texts that can describe different rules; the registry exists precisely so
//! there is one vocabulary, and this is its one renderer.

use bevy_egui::egui;
use lunco_experiments::solver::{self, SolverId};

/// Render the picker and write the selection into `selection`.
///
/// The list IS the registry, so a backend registered by any crate appears with
/// no UI edit. `None` is "Auto": [`solver::resolve`] picks the highest-ranked
/// backend that can serve the run's profile.
///
/// Returns whether the selection changed, for callers that track dirty state.
pub fn solver_picker(
    ui: &mut egui::Ui,
    id_salt: &str,
    width: f32,
    selection: &mut Option<SolverId>,
) -> bool {
    // Both entry points also resolve through the registry, but a picker can be
    // drawn before either has run — an empty combo box would read as "no solvers
    // exist" rather than "nothing registered yet".
    crate::solver_backends::ensure_builtin_solvers();

    let selected_text = selection
        .as_ref()
        .and_then(solver::get)
        .map_or_else(|| "Auto".to_string(), |spec| spec.label);

    let mut changed = false;
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(selected_text)
        .width(width)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(selection.is_none(), "Auto")
                .on_hover_text(
                    "Let the resolver pick the highest-ranked solver that can \
                     serve this run.",
                )
                .clicked()
            {
                *selection = None;
                changed = true;
            }
            for spec in solver::registered() {
                if ui
                    .selectable_label(selection.as_ref() == Some(&spec.id), &spec.label)
                    .clicked()
                {
                    *selection = Some(spec.id.clone());
                    changed = true;
                }
            }
        });
    changed
}
