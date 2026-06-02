//! Single-host modal renderer.
//!
//! Drains [`super::ModalQueue::pending`] one entry at a time and
//! renders it via `egui::Modal`, which provides the scrim,
//! pointer-event blocking, and Esc handling. Outcomes land in
//! `ModalQueue::results` for the requester to poll.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use super::{ModalBody, ModalButton, ModalOutcome, ModalQueue};

/// Renders the head of the modal queue. Runs every frame from
/// [`crate::LuncoUiPlugin`].
pub fn render_modal_host(
    mut egui_ctx: EguiContexts,
    mut queue: ResMut<ModalQueue>,
    theme: Option<Res<lunco_theme::Theme>>,
) {
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    if queue.pending.is_empty() {
        return;
    }
    let destructive_fill = theme
        .as_ref()
        .map(|t| t.tokens.error)
        .unwrap_or(egui::Color32::from_rgb(180, 60, 60));
    // Take the head; we put it back if the user didn't resolve it.
    let (id, request) = queue.pending.remove(0);
    let mut outcome: Option<ModalOutcome> = None;

    let modal_response = egui::Modal::new(egui::Id::new(("lunco_ui_modal", id))).show(ctx, |ui| {
        ui.set_max_width(420.0);
        ui.heading(&request.title);
        ui.separator();

        match &request.body {
            ModalBody::Text(text) => {
                ui.label(text);
            }
            ModalBody::Custom(paint) => {
                paint(ui);
            }
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            for button in &request.buttons {
                match button {
                    ModalButton::Confirm(label) => {
                        if ui.button(label).clicked() {
                            outcome = Some(ModalOutcome::Confirmed(label.clone()));
                        }
                    }
                    ModalButton::Cancel(label) => {
                        if ui.button(label).clicked() {
                            outcome = Some(ModalOutcome::Cancelled);
                        }
                    }
                    ModalButton::Destructive(label) => {
                        if ui
                            .add(
                                egui::Button::new(label).fill(destructive_fill),
                            )
                            .clicked()
                        {
                            outcome = Some(ModalOutcome::Destructive(label.clone()));
                        }
                    }
                }
            }
        });
    });

    // Esc / scrim click → Cancelled (only when permitted).
    if outcome.is_none() && request.dismiss_on_esc && modal_response.should_close() {
        outcome = Some(ModalOutcome::Cancelled);
    }

    match outcome {
        Some(o) => {
            queue.results.insert(id, o);
        }
        None => {
            // User hasn't acted yet — put the request back at the head.
            queue.pending.insert(0, (id, request));
        }
    }
}
