//! Shared tree-row presentation for workbench panels.
//!
//! Domain crates own their tree data, filtering, selection, and actions. This
//! module owns only the common egui branch lifecycle: the disclosure control,
//! full-width header allocation, shared row alignment and height, persistent
//! expansion state, and indented body. Keeping that contract here prevents
//! each panel from growing a slightly different tree renderer.

use egui;

/// Number of hierarchy levels that depth-based trees reveal by default.
pub const DEFAULT_OPEN_LEVELS: usize = 2;

/// Persistent disclosure state after painting one tree branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BranchState {
    /// Whether the branch body is visible after this frame's interaction.
    pub is_open: bool,
    /// Whether the disclosure state changed while this branch was painted.
    pub changed: bool,
}

/// Whether a branch at `depth` belongs to the initially visible hierarchy.
pub fn default_open_at_depth(depth: usize) -> bool {
    depth < DEFAULT_OPEN_LEVELS
}

fn row_size(ui: &egui::Ui, width: f32) -> [f32; 2] {
    [width.max(0.0), ui.spacing().interact_size.y]
}

/// Render one standard workbench tree branch and return its disclosure state.
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
///
pub fn branch(
    ui: &mut egui::Ui,
    id: egui::Id,
    default_open: bool,
    open: Option<bool>,
    add_header: impl FnOnce(&mut egui::Ui) -> bool,
    add_body: impl FnOnce(&mut egui::Ui),
) -> BranchState {
    let (state, header_rect) = paint_branch_header(ui, id, default_open, open, add_header);
    if state.is_open {
        ui.indent(id, |ui| {
            ui.expand_to_include_x(header_rect.right());
            add_body(ui);
        });
    }
    state
}

/// Paint only a branch's disclosure and header, allocating one row regardless
/// of expansion. Virtualized trees paint descendants through their flat row
/// index, so they must not allocate an indented body here. `changed` invalidates
/// that index after a user disclosure action. State and controls are shared
/// with [`branch`].
pub fn branch_header(
    ui: &mut egui::Ui,
    id: egui::Id,
    default_open: bool,
    open: Option<bool>,
    add_header: impl FnOnce(&mut egui::Ui) -> bool,
) -> BranchState {
    paint_branch_header(ui, id, default_open, open, add_header).0
}

fn paint_branch_header(
    ui: &mut egui::Ui,
    id: egui::Id,
    default_open: bool,
    open: Option<bool>,
    add_header: impl FnOnce(&mut egui::Ui) -> bool,
) -> (BranchState, egui::Rect) {
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        id,
        default_open,
    );
    let was_open = state.is_open();
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
    let is_open = state.is_open();
    state.store(ui.ctx());
    (
        BranchState {
            is_open,
            changed: was_open != is_open,
        },
        header.response.rect,
    )
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

/// Render one truncated, left-aligned text row at the shared tree-row height.
pub fn label(
    ui: &mut egui::Ui,
    text: impl Into<egui::WidgetText>,
    width: f32,
    sense: egui::Sense,
) -> egui::Response {
    ui.add_sized(
        row_size(ui, width),
        egui::Button::new(
            text.into()
                .text_style(lunco_theme::TypographyRole::Tree.text_style()),
        )
        .right_text(egui::Atom::grow())
        .truncate()
        .sense(sense)
        .frame(false),
    )
}

/// Render a full-height selectable tree label with text aligned after the
/// disclosure control, matching ordinary left-aligned tree labels.
///
/// `width` lets rows reserve space for trailing controls while keeping the
/// selectable label's typography, alignment, and height shared across trees.
pub fn selectable_label(
    ui: &mut egui::Ui,
    selected: bool,
    text: impl Into<egui::WidgetText>,
    width: f32,
) -> egui::Response {
    ui.add_sized(
        row_size(ui, width),
        egui::Button::selectable(
            selected,
            text.into()
                .text_style(lunco_theme::TypographyRole::Tree.text_style()),
        )
        .right_text(egui::Atom::grow())
        .truncate(),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn virtualized_disclosure_has_one_row_stride() {
        let context = egui::Context::default();
        let mut strides = Vec::new();
        let _ = context.run_ui(egui::RawInput::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                lunco_theme::TypographyScale::default().apply_to_style(ui.style_mut());
                let expected = ui.spacing().interact_size.y + ui.spacing().item_spacing.y;
                for open in [false, true] {
                    let start = ui.cursor().top();
                    super::branch_header(
                        ui,
                        egui::Id::new(("row", open)),
                        open,
                        Some(open),
                        |ui| {
                            super::label(ui, "branch", ui.available_width(), egui::Sense::click())
                                .clicked()
                        },
                    );
                    strides.push((expected, ui.cursor().top() - start));
                }
            });
        });
        for (expected, actual) in strides {
            assert!(
                (expected - actual).abs() < 0.01,
                "expected {expected}, allocated {actual}"
            );
        }
    }
}
