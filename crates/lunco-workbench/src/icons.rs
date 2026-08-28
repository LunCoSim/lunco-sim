//! Theme-aware vector icons for workbench controls.
//!
//! Control glyphs are painted geometry, not text. This keeps controls legible
//! when the host font has no symbol glyph and gives every UI crate one icon
//! vocabulary instead of each panel inventing a different Unicode fallback.

use bevy_egui::egui;

/// The semantic control icons shared by the workbench and its overlays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiIcon {
    /// Close or cancel.
    Close,
    /// Maximize a window.
    Maximize,
    /// Restore a maximized window.
    Restore,
    /// Minimize a window.
    Minimize,
    /// Start or resume.
    Play,
    /// Pause.
    Pause,
    /// Stop an active operation.
    Stop,
    /// Navigate backward.
    Back,
    /// Navigate forward.
    Forward,
    /// Mark a completed item.
    Check,
    /// Warning state.
    Warning,
    /// Error state.
    Error,
    /// Informational state.
    Info,
    /// Download action.
    Download,
    /// Delete action.
    Delete,
    /// Edit or rename action.
    Edit,
    /// Reset or refresh action.
    Refresh,
    /// Keyboard help.
    Keyboard,
}

/// Paint an icon into an already allocated rectangle.
pub fn paint_icon(painter: &egui::Painter, icon: UiIcon, rect: egui::Rect, color: egui::Color32) {
    // Icon controls are often wider than they are tall (for example the
    // title-bar buttons). Keep the drawing canvas square so a maximize icon
    // stays square and all control glyphs share the same visual scale. The
    // surrounding rectangle remains the full interactive area.
    let rect = icon_drawing_rect(rect);
    let stroke = egui::Stroke::new((rect.width() * 0.075).max(1.2), color);
    let inset = rect.width() * 0.2;
    let left = rect.left() + inset;
    let right = rect.right() - inset;
    let top = rect.top() + inset;
    let bottom = rect.bottom() - inset;
    let center = rect.center();

    match icon {
        UiIcon::Close | UiIcon::Error => {
            painter.line_segment([egui::pos2(left, top), egui::pos2(right, bottom)], stroke);
            painter.line_segment([egui::pos2(right, top), egui::pos2(left, bottom)], stroke);
        }
        UiIcon::Info => {
            painter.circle_stroke(center, rect.width() * 0.34, stroke);
            painter.circle_filled(
                egui::pos2(center.x, top + rect.height() * 0.22),
                stroke.width,
                color,
            );
            painter.line_segment(
                [
                    egui::pos2(center.x, top + rect.height() * 0.4),
                    egui::pos2(center.x, bottom - rect.height() * 0.16),
                ],
                stroke,
            );
        }
        UiIcon::Edit => {
            painter.line_segment(
                [
                    egui::pos2(left + rect.width() * 0.1, bottom - rect.height() * 0.1),
                    egui::pos2(right - rect.width() * 0.08, top + rect.height() * 0.12),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(left + rect.width() * 0.1, bottom - rect.height() * 0.1),
                    egui::pos2(left + rect.width() * 0.3, bottom - rect.height() * 0.18),
                ],
                stroke,
            );
        }
        UiIcon::Refresh => {
            painter.circle_stroke(center, rect.width() * 0.31, stroke);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(right, top + rect.height() * 0.34),
                    egui::pos2(right, top),
                    egui::pos2(right - rect.width() * 0.28, top),
                ],
                color,
                egui::Stroke::NONE,
            ));
        }
        UiIcon::Maximize => {
            painter.rect_stroke(
                egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, bottom)),
                0.0,
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        UiIcon::Restore => {
            let back = egui::Rect::from_min_max(
                egui::pos2(left + inset * 0.55, top),
                egui::pos2(right, bottom - inset * 0.55),
            );
            let front = egui::Rect::from_min_max(
                egui::pos2(left, top + inset * 0.55),
                egui::pos2(right - inset * 0.55, bottom),
            );
            painter.rect_stroke(back, 0.0, stroke, egui::StrokeKind::Inside);
            painter.rect_stroke(front, 0.0, stroke, egui::StrokeKind::Inside);
        }
        UiIcon::Minimize => {
            painter.line_segment(
                [egui::pos2(left, center.y), egui::pos2(right, center.y)],
                stroke,
            );
        }
        UiIcon::Play => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(left, top),
                    egui::pos2(right, center.y),
                    egui::pos2(left, bottom),
                ],
                color,
                egui::Stroke::NONE,
            ));
        }
        UiIcon::Pause => {
            let bar_w = (rect.width() * 0.18).max(2.0);
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(center.x - bar_w - bar_w * 0.55, top),
                    egui::pos2(center.x - bar_w * 0.55, bottom),
                ),
                1.0,
                color,
            );
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(center.x + bar_w * 0.55, top),
                    egui::pos2(center.x + bar_w + bar_w * 0.55, bottom),
                ),
                1.0,
                color,
            );
        }
        UiIcon::Stop => {
            painter.rect_filled(
                egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, bottom)),
                1.5,
                color,
            );
        }
        UiIcon::Back | UiIcon::Forward => {
            let forward = matches!(icon, UiIcon::Forward);
            let tip = if forward { right } else { left };
            let base = if forward { left } else { right };
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(tip, center.y),
                    egui::pos2(base, top),
                    egui::pos2(base, bottom),
                ],
                color,
                egui::Stroke::NONE,
            ));
        }
        UiIcon::Check => {
            painter.line_segment(
                [
                    egui::pos2(left, center.y),
                    egui::pos2(center.x - rect.width() * 0.06, bottom),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(center.x - rect.width() * 0.06, bottom),
                    egui::pos2(right, top),
                ],
                stroke,
            );
        }
        UiIcon::Warning => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(center.x, top),
                    egui::pos2(right, bottom),
                    egui::pos2(left, bottom),
                ],
                egui::Color32::TRANSPARENT,
                stroke,
            ));
            painter.line_segment(
                [
                    egui::pos2(center.x, top + rect.height() * 0.37),
                    egui::pos2(center.x, bottom - rect.height() * 0.2),
                ],
                stroke,
            );
            painter.circle_filled(
                egui::pos2(center.x, bottom - rect.height() * 0.1),
                stroke.width,
                color,
            );
        }
        UiIcon::Download => {
            painter.line_segment(
                [
                    egui::pos2(center.x, top),
                    egui::pos2(center.x, bottom - rect.height() * 0.2),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(center.x - rect.width() * 0.2, center.y),
                    egui::pos2(center.x, bottom - rect.height() * 0.2),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(center.x + rect.width() * 0.2, center.y),
                    egui::pos2(center.x, bottom - rect.height() * 0.2),
                ],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(left, bottom), egui::pos2(right, bottom)],
                stroke,
            );
        }
        UiIcon::Delete => {
            painter.rect_stroke(
                egui::Rect::from_min_max(
                    egui::pos2(left + rect.width() * 0.08, top + rect.height() * 0.12),
                    egui::pos2(right - rect.width() * 0.08, bottom),
                ),
                1.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.line_segment([egui::pos2(left, top), egui::pos2(right, top)], stroke);
            painter.line_segment(
                [
                    egui::pos2(center.x - rect.width() * 0.14, top),
                    egui::pos2(center.x + rect.width() * 0.14, top),
                ],
                stroke,
            );
        }
        UiIcon::Keyboard => {
            painter.rect_stroke(
                egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, bottom)),
                2.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            for column in 0..3 {
                for row in 0..2 {
                    let x = left + rect.width() * (0.22 + column as f32 * 0.22);
                    let y = top + rect.height() * (0.36 + row as f32 * 0.27);
                    painter.circle_filled(egui::pos2(x, y), stroke.width * 0.7, color);
                }
            }
        }
    }
}

fn icon_drawing_rect(rect: egui::Rect) -> egui::Rect {
    let side = rect.width().min(rect.height());
    egui::Rect::from_center_size(rect.center(), egui::vec2(side, side))
}

/// Allocate and paint a compact icon-only button with an accessible tooltip.
pub fn icon_button(ui: &mut egui::Ui, icon: UiIcon, tooltip: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(28.0, 24.0), egui::Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 4.0, ui.visuals().widgets.hovered.bg_fill);
    }
    paint_icon(
        ui.painter(),
        icon,
        rect.shrink(2.0),
        ui.visuals().widgets.inactive.fg_stroke.color,
    );
    response.on_hover_text(tooltip)
}

/// Allocate a normal text button with a painted semantic icon on its left.
pub fn icon_text_button(
    ui: &mut egui::Ui,
    icon: UiIcon,
    label: &str,
    tooltip: &str,
) -> egui::Response {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        font,
        ui.visuals().widgets.inactive.fg_stroke.color,
    );
    let size = egui::vec2(galley.size().x + 36.0, galley.size().y + 8.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let visuals = ui.visuals();
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 4.0, visuals.widgets.hovered.bg_fill);
    }
    let color = if response.hovered() {
        visuals.widgets.hovered.fg_stroke.color
    } else {
        visuals.widgets.inactive.fg_stroke.color
    };
    paint_icon(
        ui.painter(),
        icon,
        egui::Rect::from_min_size(
            rect.left_top() + egui::vec2(4.0, 4.0),
            egui::vec2(20.0, 20.0),
        ),
        color,
    );
    ui.painter().galley(
        egui::pos2(rect.left() + 28.0, rect.center().y - galley.size().y * 0.5),
        galley,
        color,
    );
    response.on_hover_text(tooltip)
}

#[cfg(test)]
mod tests {
    use super::icon_drawing_rect;
    use bevy_egui::egui;

    #[test]
    fn icon_canvas_is_square_and_centered_in_titlebar_button() {
        let button = egui::Rect::from_min_size(egui::pos2(100.0, 40.0), egui::vec2(28.0, 24.0));
        let canvas = icon_drawing_rect(button.shrink(2.0));

        assert_eq!(canvas.size(), egui::vec2(20.0, 20.0));
        assert_eq!(canvas.center(), button.center());
    }
}
