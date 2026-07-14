//! Sky clock control (doc 19 §11b) — the sandbox's celestial-time panel.
//!
//! The workbench (`luncosim`) has a "Time Control" panel, but it drives
//! `TimeTransport`: the *simulation* transport, which pauses physics and the tick.
//! That is a different thing from the **celestial clock**, and conflating them is
//! what made "speed up time to watch the Earth move" also fast-forward the rovers.
//!
//! This panel drives the celestial clock alone, via [`SetClock`]:
//!
//! * **Follow sim** — the clock hangs under the sim clock: pausing the world freezes
//!   the sky too (the default, and the deterministic/replay-safe one).
//! * **Independent** — the clock is re-parented onto the wall root, so the sky keeps
//!   running at its own rate **while the simulation is paused**. A clock is frozen
//!   because of *where it hangs*, so running one anyway is a re-parent, not a flag.
//! * **Rate** — `scale` on that clock. `1000×` moves the Earth across the lunar sky
//!   in a couple of minutes; the sim is untouched either way.
//!
//! Only drawn when the scene actually declared celestial bodies (§11e) — no sky, no
//! sky clock.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use lunco_core::CelestialBody;
use lunco_time::{ClockId, ClockParent, Clocks, SetClock, TimeDomain, WorldTime};

/// Paint the sky-clock pill (top-left, under the view switcher) and dispatch
/// [`SetClock`]. Runs in `EguiPrimaryContextPass`; early-outs when the scene has no
/// celestial bodies.
pub(crate) fn draw_celestial_time(
    mut egui_ctx: EguiContexts,
    q_bodies: Query<(), With<CelestialBody>>,
    clocks: Option<Res<Clocks>>,
    q_domains: Query<&TimeDomain>,
    world: Option<Res<WorldTime>>,
    mut commands: Commands,
) {
    if q_bodies.is_empty() {
        return;
    }
    let (Some(clocks), Some(world)) = (clocks, world) else { return };
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };

    // The sky is "independent" exactly when its clock hangs off the wall root.
    let domain = q_domains.get(clocks.celestial).ok();
    let independent = domain.is_some_and(|d| d.parent == Some(clocks.real));
    let scale = domain.map(|d| d.scale).unwrap_or(1.0);

    egui::Area::new(egui::Id::new("celestial_time"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::LEFT_TOP, egui::vec2(12.0, 40.0))
        .interactable(true)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Sky").weak().size(11.0));
                        ui.label(
                            egui::RichText::new(world.utc_string())
                                .monospace()
                                .size(11.0),
                        )
                        .on_hover_text(format!("JD {:.4} (TDB)", world.epoch_jd));
                    });

                    ui.horizontal(|ui| {
                        // Coupling: this is the pause story. "Follow sim" hangs the
                        // clock under the tick master (freezes with the world);
                        // "Independent" re-parents it onto the wall clock so the sky
                        // runs even while the simulation is paused.
                        let follow = ui
                            .selectable_label(!independent, "Follow sim")
                            .on_hover_text(
                                "The sky is part of the simulation: pausing the world \
                                 freezes it too. Deterministic and replay-safe.",
                            );
                        if follow.clicked() && independent {
                            commands.trigger(SetClock {
                                clock: ClockId::Celestial,
                                parent: Some(ClockParent::Sim),
                                scale: Some(1.0),
                                ..default()
                            });
                        }
                        let indep = ui
                            .selectable_label(independent, "Independent")
                            .on_hover_text(
                                "Run the sky on its own clock — it keeps moving while \
                                 the simulation is paused.",
                            );
                        if indep.clicked() && !independent {
                            commands.trigger(SetClock {
                                clock: ClockId::Celestial,
                                parent: Some(ClockParent::Real),
                                ..default()
                            });
                        }
                    });

                    ui.horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new("Rate").weak().size(11.0));
                        // 1× is realtime, which on a lunar day is imperceptible — the
                        // useful range for watching Earth cross the sky starts around
                        // 1000×. The simulation's own rate is untouched by these.
                        for m in [1.0_f64, 100.0, 1_000.0, 10_000.0, 100_000.0] {
                            let label = if m >= 1000.0 {
                                format!("{}k×", m / 1000.0)
                            } else {
                                format!("{m}×")
                            };
                            if ui
                                .selectable_label((scale - m).abs() < f64::EPSILON, label)
                                .clicked()
                            {
                                commands.trigger(SetClock {
                                    clock: ClockId::Celestial,
                                    scale: Some(m),
                                    ..default()
                                });
                            }
                        }
                    });

                    if independent {
                        ui.label(
                            egui::RichText::new("sky detached from sim")
                                .weak()
                                .size(10.0),
                        );
                    }
                });
        });
}
