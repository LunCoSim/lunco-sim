//! Shared tree-row presentation for workbench panels.
//!
//! Domain crates own their tree data, filtering, selection, and actions. This
//! module owns only the common egui branch lifecycle: the disclosure control,
//! full-width header allocation, persistent expansion state, and indented
//! body. Keeping that contract here prevents each panel from growing a
//! slightly different tree renderer.

use bevy_egui::egui;

/// Render one standard workbench tree branch.
///
/// `add_header` paints the branch contents after the shared disclosure
/// control and returns whether its label was clicked. A label click toggles
/// the branch, matching egui's standard collapsing-header interaction. The
/// callback may also collect a domain-specific action; that action remains
/// the caller's responsibility.
///
/// When `open` is `Some`, the branch is controlled for this frame (useful for
/// active search results) and cannot be closed by a user click until the
/// caller stops supplying the forced value.
pub fn branch(
    ui: &mut egui::Ui,
    id: egui::Id,
    default_open: bool,
    open: Option<bool>,
    add_header: impl FnOnce(&mut egui::Ui) -> bool,
    add_body: impl FnOnce(&mut egui::Ui),
) {
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        id,
        default_open,
    );
    let mut label_clicked = false;
    let header = ui.horizontal(|ui| {
        let item_spacing = ui.spacing().item_spacing;
        ui.spacing_mut().item_spacing.x = 0.0;
        state.show_toggle_button(ui, egui::collapsing_header::paint_default_icon);
        ui.spacing_mut().item_spacing = item_spacing;

        // A tree header is a row, not a content-sized label. This keeps
        // hover/click geometry consistent across every tree consumer.
        let available_width = ui.available_width();
        ui.set_min_width(available_width);
        label_clicked = add_header(ui);
    });

    if open.is_none() && label_clicked {
        state.toggle(ui);
    }
    if let Some(open) = open {
        state.set_open(open);
    }
    if state.is_open() {
        ui.indent(id, |ui| {
            ui.expand_to_include_x(header.response.rect.right());
            add_body(ui);
        });
    }
    state.store(ui.ctx());
}

/// Render one full-width leaf row using the same horizontal allocation as a
/// [`branch`]'s header.
pub fn leaf<R>(
    ui: &mut egui::Ui,
    add_row: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let available_width = ui.available_width();
    ui.horizontal(|ui| {
        ui.set_min_width(available_width);
        add_row(ui)
    })
}
