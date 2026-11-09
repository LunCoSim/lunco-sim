//! Modelica workbench panels.

/// Shared size for primary actions on Modelica start surfaces.
pub(super) const START_ACTION_SIZE: [f32; 2] = [272.0, 44.0];

/// Render the paired primary actions used by the Modelica start surfaces.
/// They share a width and stack when the panel is too narrow for one row.
pub(super) fn render_start_actions(
    ui: &mut bevy_egui::egui::Ui,
    primary: (&str, &str),
    secondary: (&str, &str),
) -> (bool, bool) {
    let available_width = ui.available_width().max(0.0);
    let width = available_width.min(START_ACTION_SIZE[0]);
    let row_width = width * 2.0 + ui.spacing().item_spacing.x;
    if row_width <= available_width {
        ui.horizontal(|ui| {
            (
                render_start_action(ui, primary, width),
                render_start_action(ui, secondary, width),
            )
        })
        .inner
    } else {
        ui.vertical_centered(|ui| {
            (
                render_start_action(ui, primary, width),
                render_start_action(ui, secondary, width),
            )
        })
        .inner
    }
}

fn render_start_action(ui: &mut bevy_egui::egui::Ui, action: (&str, &str), width: f32) -> bool {
    ui.add_sized(
        [width, START_ACTION_SIZE[1]],
        bevy_egui::egui::Button::new(
            bevy_egui::egui::RichText::new(action.0)
                .text_style(lunco_theme::TypographyRole::Button.text_style())
                .strong(),
        ),
    )
    .on_hover_text(action.1)
    .clicked()
}

pub mod canvas_diagram;
pub mod canvas_projection;
pub mod code_editor;
pub mod console;
pub mod csv_export;
pub mod diagnostics;
pub mod experiments;
pub mod graphs;
pub mod inspector;
pub mod journal;
pub mod model_view;
pub mod package_browser;
pub mod palette;
pub mod placeholder;
pub mod telemetry;
pub mod welcome;
pub mod workspace;
