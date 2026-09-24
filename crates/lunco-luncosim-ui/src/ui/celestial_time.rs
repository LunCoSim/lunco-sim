//! Sky clock control (doc 19 §11b) — the luncosim's celestial-time panel.
//!
//! CelestialTime is a scaled child of WorldTime. The selected rate advances all
//! celestial state and model inputs from one epoch; pausing WorldTime freezes it.
//!
//! Only drawn when the scene actually declared celestial bodies (§11e) — no sky, no
//! sky clock.
//!
//! Two surfaces, ONE body of controls ([`sky_clock_ui`]):
//!
//! * the workbench **Time** menu, always available (`sky_clock_menu_ui`);
//! * the floating pill, which is OFF by default and opted into via
//!   [`OverlaySettings`](super::overlays::OverlaySettings).
//!
//! The overlay is a convenience, not the only way in — hiding it must not take the
//! celestial clock with it.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use lunco_celestial::CelestialBody;
use lunco_time::{CelestialTime, Clocks, SetCelestialClock, TimeDomain};
use lunco_workbench_core::MenuCtx;

fn sky_clock_state(clocks: Clocks, domain: Option<TimeDomain>) -> Option<f64> {
    let domain = domain?;
    (domain.parent == Some(clocks.sim)).then_some(domain.scale)
}

/// The sky-clock controls, drawn into whatever `Ui` is given.
///
/// Takes the state it needs rather than queries, so the same widget serves a
/// system (which has `Res`/`Query`) and a menu callback.
/// Returns the [`SetCelestialClock`] the user asked for, if any — the caller owns dispatch,
/// because triggering differs between the two contexts.
fn sky_clock_ui(
    ui: &mut egui::Ui,
    utc: &str,
    epoch_jd: f64,
    scale: f64,
) -> Option<SetCelestialClock> {
    let mut request = None;

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Sky").weak().size(11.0));
        ui.label(egui::RichText::new(utc).monospace().size(11.0))
            .on_hover_text(format!("JD {epoch_jd:.4} (TDB)"));
    });

    // ── Seek to a date ────────────────────────────────────────────────────
    //
    // The buffer lives in egui memory keyed by the widget id, not in a `Local`:
    // this body is drawn from BOTH the Time menu and the floating pill, and a
    // per-caller `Local` would give the two surfaces different half-typed text.
    // Seeded from the displayed time, so opening it shows where you are and the
    // string is already in the format it accepts.
    let buf_id = egui::Id::new("sky_clock_seek_buf");
    let mut buf: String = ui
        .data(|d| d.get_temp::<String>(buf_id))
        .unwrap_or_else(|| utc.trim_end_matches(" UTC").to_string());

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Go to").weak().size(11.0));
        let parsed = lunco_time::utc_string_to_tdb_jd(&buf);
        let field = lunco_workbench_widgets::text_editor::singleline(&mut buf)
            .desired_width(172.0)
            .font(egui::TextStyle::Monospace)
            // Invalid text is marked, never silently ignored: a seek that does
            // nothing and says nothing reads as a broken clock.
            .text_color_opt(
                parsed
                    .is_none()
                    .then_some(egui::Color32::from_rgb(220, 120, 120)),
            );
        let resp = ui.add(field).on_hover_text(
            "UTC date to put the celestial clock at — `YYYY-MM-DD HH:MM:SS`, `YYYY-MM-DD HH:MM` \
             or `YYYY-MM-DD`. All celestial consumers read this same epoch.",
        );
        let entered = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let go = ui
            .add_enabled(parsed.is_some(), egui::Button::new("Set"))
            .on_disabled_hover_text("Not a date this understands — see the field's tooltip.")
            .clicked();
        if let (Some(jd), true) = (parsed, go || entered) {
            request = Some(SetCelestialClock {
                epoch_jd: Some(jd),
                ..default()
            });
        }
        if ui
            .button("Now")
            .on_hover_text("Seek the shared celestial clock to the current UTC date.")
            .clicked()
        {
            let now = lunco_time::utc_now_tdb_jd();
            buf = lunco_time::tdb_jd_to_utc_string(now)
                .trim_end_matches(" UTC")
                .to_string();
            request = Some(SetCelestialClock {
                epoch_jd: Some(now),
                ..default()
            });
        }
    });
    ui.data_mut(|d| d.insert_temp(buf_id, buf));

    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("Rate").weak().size(11.0));
        // The celestial epoch scales from WorldTime. Physics retains its fixed
        // timestep; models consume the shared celestial sample at that cadence.
        for m in [
            1.0_f64,
            100.0,
            1_000.0,
            10_000.0,
            lunco_time::MAX_CELESTIAL_TIME_RATE,
        ] {
            let label = if m >= 1000.0 {
                format!("{}k×", m / 1000.0)
            } else {
                format!("{m}×")
            };
            if ui
                .selectable_label((scale - m).abs() < f64::EPSILON, label)
                .on_hover_text("Scale the shared celestial epoch relative to WorldTime.")
                .clicked()
            {
                request = Some(SetCelestialClock {
                    scale: Some(m),
                    ..default()
                });
            }
        }
    });

    request
}

/// The sky clock as a **Time menu** section. Draws nothing when the scene declared
/// no celestial bodies — no sky, no sky clock, the same rule the overlay follows.
pub(crate) fn sky_clock_menu_ui(ui: &mut egui::Ui, ctx: &mut MenuCtx) {
    if !ctx.has_component::<CelestialBody>() {
        ui.label(egui::RichText::new("No sky in this scene").weak().small());
        return;
    }
    let (Some(clocks), Some(celestial)) = (
        ctx.resource::<Clocks>().copied(),
        ctx.resource::<CelestialTime>().copied(),
    ) else {
        return;
    };
    let state = sky_clock_state(clocks, ctx.get::<TimeDomain>(clocks.celestial).copied());
    let Some(scale) = state else {
        ui.label(
            egui::RichText::new("Celestial clock unavailable")
                .weak()
                .small(),
        );
        return;
    };

    let utc = lunco_time::tdb_jd_to_utc_string(celestial.epoch_jd);
    if let Some(req) = sky_clock_ui(ui, &utc, celestial.epoch_jd, scale) {
        ctx.trigger(req);
    }
}

/// Paint the sky-clock pill (top-left, under the view switcher) and dispatch
/// [`SetCelestialClock`]. Runs in `EguiPrimaryContextPass`; early-outs when the scene has no
/// celestial bodies.
pub(crate) fn draw_celestial_time(
    mut egui_ctx: EguiContexts,
    q_bodies: Query<(), With<CelestialBody>>,
    clocks: Option<Res<Clocks>>,
    q_domains: Query<&TimeDomain>,
    celestial: Option<Res<CelestialTime>>,
    mut commands: Commands,
) {
    if q_bodies.is_empty() {
        return;
    }
    let (Some(clocks), Some(celestial)) = (clocks, celestial) else {
        return;
    };
    let utc = lunco_time::tdb_jd_to_utc_string(celestial.epoch_jd);
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };

    let state = sky_clock_state(*clocks, q_domains.get(clocks.celestial).ok().copied());
    let Some(scale) = state else {
        bevy::log::warn_once!("[ui] celestial clock is unavailable; hiding its overlay");
        return;
    };

    egui::Area::new(egui::Id::new("celestial_time"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::LEFT_TOP, egui::vec2(12.0, 40.0))
        .interactable(true)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    if let Some(req) = sky_clock_ui(ui, &utc, celestial.epoch_jd, scale) {
                        commands.trigger(req);
                    }
                });
        });
}
