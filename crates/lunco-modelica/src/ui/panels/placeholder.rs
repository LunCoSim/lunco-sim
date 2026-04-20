//! Shared "centered card" overlay used by any view that needs to
//! show a message over empty-ish content (empty Diagram, missing
//! Icon, future: missing Documentation).
//!
//! # Why a shared helper
//!
//! Multiple views used to each do their own `Rect::from_center_size`
//! + `new_child` layout. Small diffs between them meant the Icon
//! fallback looked noticeably off-centre compared to the Diagram
//! empty-state card. Consolidating here keeps visual parity and
//! stops the centering from drifting apart as features get added.
//!
//! # Contract
//!
//! - `target_rect` is the rectangle to center the card inside —
//!   typically `ui.available_rect_before_wrap()` for a fresh panel
//!   body, or the response rect from a painted widget (e.g. the
//!   Canvas's `response.rect`).
//! - Card is always drawn via direct `painter` calls so it overlays
//!   any content already painted underneath (no widget allocation,
//!   no layout reflow).
//! - Content is rendered via a child UI with `Layout::top_down(Align::Center)`
//!   inside `card_rect.shrink(16.0)`, so the caller just emits labels
//!   and they stack centred automatically.

use bevy_egui::egui;

const CARD_CORNER_RADIUS: f32 = 10.0;
const CARD_SHADOW_OFFSET_Y: f32 = 3.0;
const CARD_SHADOW_ALPHA: u8 = 100;
const CARD_PADDING: f32 = 16.0;

/// Draw a centred rounded card with a drop shadow at the centre of
/// `target_rect`, then hand a child `Ui` to `content` so the caller
/// can stack labels inside it.
///
/// ```ignore
/// placeholder::render_centered_card(ui, rect, egui::vec2(340.0, 120.0), |ui| {
///     ui.label("No icon defined");
///     ui.label("Add an annotation(Icon(...)) clause.");
/// });
/// ```
pub fn render_centered_card(
    ui: &mut egui::Ui,
    target_rect: egui::Rect,
    card_size: egui::Vec2,
    theme: &lunco_theme::Theme,
    content: impl FnOnce(&mut egui::Ui),
) {
    let card_rect = egui::Rect::from_center_size(target_rect.center(), card_size);
    let fill = theme.colors.surface0;
    let stroke_color = theme.colors.surface2;
    let shadow_src = theme.colors.base;
    let shadow = egui::Color32::from_rgba_unmultiplied(
        shadow_src.r(),
        shadow_src.g(),
        shadow_src.b(),
        CARD_SHADOW_ALPHA,
    );

    let painter = ui.painter();
    painter.rect_filled(
        card_rect.translate(egui::vec2(0.0, CARD_SHADOW_OFFSET_Y)),
        CARD_CORNER_RADIUS,
        shadow,
    );
    painter.rect_filled(card_rect, CARD_CORNER_RADIUS, fill);
    painter.rect_stroke(
        card_rect,
        CARD_CORNER_RADIUS,
        egui::Stroke::new(1.0, stroke_color),
        egui::StrokeKind::Outside,
    );

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(card_rect.shrink(CARD_PADDING))
            .layout(egui::Layout::top_down(egui::Align::Center)),
    );
    content(&mut child);
}
