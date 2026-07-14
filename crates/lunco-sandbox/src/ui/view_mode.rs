//! Viewport view-mode switcher: Surface ⇄ Moon ⇄ Earth.
//!
//! Visible only while the celestial hierarchy is live — i.e. when the scene declared
//! celestial bodies in USD (`LuncoCelestialBodyAPI`, doc 19 §11e), so plain sandbox
//! scenes never show the pill. Pure dispatch, no new machinery:
//! Moon/Earth trigger the existing `FocusTarget` (doc 47 Phase 6 orbital view —
//! the camera travels, the world never re-poses); Surface triggers the existing
//! `ReleaseVessel`, the one canonical unwind (same as Backspace/Cancel), whose
//! orbital branch restores the camera pose parked on mode entry.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use lunco_avatar::{FocusTarget, ReleaseVessel};
use lunco_celestial::OrbitalViewPin;
use lunco_core::{Avatar, CelestialBody};

const MOON_NAIF: i32 = 301;
const EARTH_NAIF: i32 = 399;

/// Paint the switcher pill top-center and dispatch the typed camera commands.
/// Runs in `EguiPrimaryContextPass`; early-outs when no celestial bodies exist.
pub(crate) fn draw_view_mode_switcher(
    mut egui_ctx: EguiContexts,
    q_bodies: Query<(Entity, &CelestialBody)>,
    q_avatar: Query<(Entity, Option<&Camera>), With<Avatar>>,
    orbital_pin: Option<Res<OrbitalViewPin>>,
    mut commands: Commands,
) {
    let mut moon = None;
    let mut earth = None;
    for (e, body) in q_bodies.iter() {
        match body.ephemeris_id {
            MOON_NAIF => moon = Some(e),
            EARTH_NAIF => earth = Some(e),
            _ => {}
        }
    }
    if moon.is_none() && earth.is_none() {
        return;
    }
    let Ok(ctx) = egui_ctx.ctx_mut() else {
        return;
    };

    let orbital_body = orbital_pin.filter(|p| p.active).map(|p| p.body);

    egui::Area::new(egui::Id::new("view_mode_switcher"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 40.0))
        .interactable(true)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("View").weak().size(11.0));
                        let surface = ui
                            .selectable_label(orbital_body.is_none(), "Surface")
                            .on_hover_text("Return to the surface view you left");
                        if surface.clicked() && orbital_body.is_some() {
                            // Same avatar preference as `on_focus_command`: the
                            // one carrying the ACTIVE render camera, not a scene
                            // spawn-point Avatar prim.
                            let avatar = q_avatar
                                .iter()
                                .find(|(_, cam)| cam.is_some_and(|c| c.is_active))
                                .or_else(|| q_avatar.iter().next())
                                .map(|(e, _)| e);
                            if let Some(target) = avatar {
                                commands.trigger(ReleaseVessel { target });
                            }
                        }
                        for (label, naif, target, hover) in [
                            ("Moon", MOON_NAIF, moon, "Orbital view of the Moon"),
                            ("Earth", EARTH_NAIF, earth, "Orbital view of Earth"),
                        ] {
                            let selected = orbital_body == Some(naif);
                            match target {
                                Some(target) => {
                                    let resp =
                                        ui.selectable_label(selected, label).on_hover_text(hover);
                                    if resp.clicked() && !selected {
                                        commands.trigger(FocusTarget { avatar: None, target });
                                    }
                                }
                                None => {
                                    ui.add_enabled_ui(false, |ui| {
                                        ui.selectable_label(false, label)
                                            .on_disabled_hover_text(
                                                "Body not present in this scene",
                                            );
                                    });
                                }
                            }
                        }
                    });
                });
        });
}
