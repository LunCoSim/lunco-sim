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

use bevy::prelude::*;
use bevy_egui::egui;

/// Visual constants for the card. Matches the previous
/// canvas-diagram empty-state look so we don't regress that view.
struct CardStyle {
    corner_radius: f32,
    shadow_offset_y: f32,
    shadow_alpha: u8,
    fill: egui::Color32,
    stroke_color: egui::Color32,
    padding: f32,
}

const DEFAULT_STYLE: CardStyle = CardStyle {
    corner_radius: 10.0,
    shadow_offset_y: 3.0,
    shadow_alpha: 100,
    fill: egui::Color32::from_rgb(34, 38, 48),
    stroke_color: egui::Color32::from_rgb(60, 70, 88),
    padding: 16.0,
};

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
    content: impl FnOnce(&mut egui::Ui),
) {
    let style = &DEFAULT_STYLE;
    let card_rect = egui::Rect::from_center_size(target_rect.center(), card_size);

    let painter = ui.painter();
    // Drop shadow — drawn first, offset downward.
    painter.rect_filled(
        card_rect.translate(egui::vec2(0.0, style.shadow_offset_y)),
        style.corner_radius,
        egui::Color32::from_rgba_premultiplied(0, 0, 0, style.shadow_alpha),
    );
    painter.rect_filled(card_rect, style.corner_radius, style.fill);
    painter.rect_stroke(
        card_rect,
        style.corner_radius,
        egui::Stroke::new(1.0, style.stroke_color),
        egui::StrokeKind::Outside,
    );

    // Content via child UI. `Align::Center` on a top-down layout
    // horizontally centres each emitted label; vertical centring is
    // implicit because caller-emitted content stacks from the top
    // of the padded card and the card itself is already in the
    // visual middle of `target_rect`.
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(card_rect.shrink(style.padding))
            .layout(egui::Layout::top_down(egui::Align::Center)),
    );
    content(&mut child);
}
