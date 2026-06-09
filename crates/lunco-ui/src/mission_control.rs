//! Mission Control panel — single unified panel for time, bodies, spacecraft, rovers, and actions.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};
use chrono::TimeZone;

use lunco_core::{Avatar, RoverVessel, Spacecraft, CelestialClock};
use lunco_celestial::{CelestialBody, TeleportToSurface, LeaveSurface};
use lunco_avatar::{PossessVessel, ReleaseVessel, FocusTarget};

/// Mission Control panel — everything in one place.
pub struct MissionControl;

impl Panel for MissionControl {
    fn id(&self) -> PanelId { PanelId("mission_control") }
    fn title(&self) -> String { "Mission Control".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::RightInspector }
    fn transparent_background(&self) -> bool { true }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        let theme = world.resource::<lunco_theme::Theme>().clone();
        ui.style_mut().visuals = theme.to_visuals();

        egui::Frame::new()
            .fill(theme.colors.mantle)
            .inner_margin(8.0)
            .corner_radius(4)
            .show(ui, |ui| {
                self.render_content(ui, world);
            });
    }
}

impl MissionControl {
    fn render_content(&mut self, ui: &mut egui::Ui, world: &mut World) {

        let avatar_ent = {
            let mut q = world.query_filtered::<Entity, With<Avatar>>();
            q.iter(world).next()
        };

        // ── Time Control ──
        ui.heading("Time Control");
        if let Some(clock) = world.get_resource::<CelestialClock>() {
            ui.label(format!("JD: {:.4}", clock.epoch));
            ui.label(format!("UTC: {}", jd_to_utc_string(clock.epoch)));
        }
        if let Some(mut clock) = world.get_resource_mut::<CelestialClock>() {
            ui.horizontal(|ui| {
                if ui.button(if clock.paused { "▶ Play" } else { "⏸ Pause" }).clicked() {
                    clock.paused = !clock.paused;
                }
            });
            egui::Grid::new("time_multipliers")
                .num_columns(4)
                .spacing([4.0, 4.0])
                .show(ui, |ui| {
                    let multipliers = [1.0, 10.0, 100.0, 1000.0, 10000.0, 100000.0, 1000000.0];
                    for (i, &m) in multipliers.iter().enumerate() {
                        if ui.selectable_label(clock.speed_multiplier == m, format!("{}x", m)).clicked() {
                            clock.speed_multiplier = m;
                        }
                        if (i + 1) % 4 == 0 {
                            ui.end_row();
                        }
                    }
                });
        }
        ui.separator();

        // ── Celestial Bodies ──
        ui.collapsing("Celestial Bodies", |ui| {
            let mut body_q = world.query::<(Entity, &Name, &CelestialBody)>();
            let bodies: Vec<(Entity, String, String)> = body_q.iter(world)
                .map(|(e, n, body)| (e, n.as_str().to_string(), format!("{:.0} km", body.radius_m / 1000.0)))
                .collect();
            for (entity, name, radius) in &bodies {
                ui.horizontal(|ui| {
                    ui.label(format!("{} ({})", name, radius));
                    if ui.small_button("Focus").clicked() {
                        if let Some(av) = avatar_ent {
                            world.commands().trigger(FocusTarget { avatar: Some(av), target: *entity });
                        }
                    }
                    if ui.small_button("🌕 Surface").clicked() {
                        if let Some(av) = avatar_ent {
                            world.commands().trigger(TeleportToSurface {
                                target: av,
                                body_entity: entity.to_bits(),
                            });
                        }
                    }
                });
            }
        });

        // ── Spacecraft ──
        ui.collapsing("Spacecraft", |ui| {
            let mut sc_q = world.query::<(Entity, &Name)>();
            let scs: Vec<(Entity, String)> = sc_q.iter(world)
                .filter(|(e, _)| world.get::<Spacecraft>(*e).is_some())
                .map(|(e, n)| (e, n.as_str().to_string()))
                .collect();
            for (entity, name) in &scs {
                ui.horizontal(|ui| {
                    ui.label(name);
                    if ui.small_button("Focus").clicked() {
                        if let Some(av) = avatar_ent {
                            world.commands().trigger(FocusTarget { avatar: Some(av), target: *entity });
                        }
                    }
                });
            }
        });

        // ── Rovers ──
        ui.collapsing("Rovers", |ui| {
            // Networking context, read from the always-on substrate. In
            // single-player (Standalone, empty registry) `networked` is false so
            // the ownership UI stays hidden and rovers behave as before.
            let local_session = world.get_resource::<lunco_core::LocalSession>().map(|l| l.0);
            let (networked, is_host) = world
                .get_resource::<lunco_core::NetworkRole>()
                .map(|r| (r.is_networked(), r.is_host()))
                .unwrap_or((false, false));

            // Possession policy selector (host-authoritative — only the host's
            // choice governs `claim`). Default `Exclusive` = "one each".
            let policy = world
                .get_resource::<lunco_core::SessionRegistry>()
                .map(|r| r.policy())
                .unwrap_or_default();
            if networked {
                let mut chosen = policy;
                ui.horizontal(|ui| {
                    ui.label("Control:");
                    ui.add_enabled_ui(is_host, |ui| {
                        ui.selectable_value(
                            &mut chosen,
                            lunco_core::PossessionPolicy::Exclusive,
                            "One each",
                        );
                        ui.selectable_value(
                            &mut chosen,
                            lunco_core::PossessionPolicy::LastWins,
                            "Anyone",
                        );
                    });
                });
                if chosen != policy && is_host {
                    if let Some(mut reg) = world.get_resource_mut::<lunco_core::SessionRegistry>() {
                        reg.set_policy(chosen);
                    }
                }
            }

            let mut rover_q = world.query::<(Entity, &Name)>();
            let rovers: Vec<(Entity, String, Option<u64>)> = rover_q.iter(world)
                .filter(|(e, _)| world.get::<RoverVessel>(*e).is_some())
                .map(|(e, n)| {
                    (e, n.as_str().to_string(),
                     world.get::<lunco_core::GlobalEntityId>(e).map(|g| g.get()))
                })
                .collect();

            // Resolve owner per rover (registry borrow kept out of the query iter).
            let owners: Vec<Option<lunco_core::SessionId>> = {
                let reg = world.get_resource::<lunco_core::SessionRegistry>();
                rovers.iter()
                    .map(|(_, _, gid)| gid.and_then(|g| reg.and_then(|r| r.owner_of(g))))
                    .collect()
            };

            for ((entity, name, _gid), owner) in rovers.iter().zip(owners.iter()) {
                let mine = matches!((owner, local_session), (Some(o), Some(l)) if *o == l);
                let taken_by_other = owner.is_some() && !mine;
                ui.horizontal(|ui| {
                    // Ownership chip (only meaningful with the wire live).
                    if networked {
                        if mine {
                            ui.colored_label(egui::Color32::from_rgb(0x4c, 0xff, 0x88), "●")
                                .on_hover_text("You control this rover");
                        } else if taken_by_other {
                            ui.colored_label(egui::Color32::from_rgb(0xff, 0x6b, 0x4c), "🔒")
                                .on_hover_text(format!(
                                    "Controlled by session {}",
                                    owner.map(|s| s.0).unwrap_or(0)
                                ));
                        } else {
                            ui.weak("○").on_hover_text("Free to possess");
                        }
                    }
                    ui.label(name);
                    if ui.small_button("Focus").clicked() {
                        if let Some(av) = avatar_ent {
                            world.commands().trigger(FocusTarget { avatar: Some(av), target: *entity });
                        }
                    }
                    if mine {
                        if ui.small_button("🚪 Release").clicked() {
                            if let Some(av) = avatar_ent {
                                world.commands().trigger(ReleaseVessel { target: av });
                            }
                        }
                    } else {
                        // Under `Exclusive` a rover held by another is locked;
                        // under `LastWins` anyone can take it (button stays live).
                        let locked = taken_by_other
                            && matches!(policy, lunco_core::PossessionPolicy::Exclusive);
                        let resp =
                            ui.add_enabled(!locked, egui::Button::new("🚗 Possess").small());
                        if locked {
                            resp.on_disabled_hover_text(
                                "Controlled by another player (One-each policy)",
                            );
                        } else if resp.clicked() {
                            if let Some(av) = avatar_ent {
                                world.commands().trigger(PossessVessel { avatar: Some(av), target: *entity });
                            }
                        }
                    }
                });
            }
        });

        // ── Quick Actions ──
        ui.separator();
        ui.heading("Quick Actions");
        if let Some(av) = avatar_ent {
            if ui.button("🚀 Release (Free Fly)").clicked() {
                world.commands().trigger(ReleaseVessel { target: av });
            }

            // Return to Orbit — show when avatar is in surface mode
            let on_surface = {
                let mut q = world.query::<&lunco_avatar::SurfaceCamera>();
                q.iter(world).next().is_some()
            };
            if on_surface {
                if ui.button("🏠 Return to Orbit").clicked() {
                    let target = world.get_resource::<lunco_celestial::LocalGravityField>()
                        .and_then(|gf| gf.body_entity);
                    if target.is_some() {
                        world.commands().trigger(LeaveSurface { target: av });
                    }
                }
            }
        }

        ui.separator();
        ui.label("Double-click entities in Inspector to focus.");
        ui.label("WASD: move  |  QE: Up/Down");
        ui.label("Right-Click: rotate  |  SPACE: pause");
    }
}

fn jd_to_utc_string(jd: f64) -> String {
    let j2000 = 2451545.0;
    let days_since_j2000 = (jd - j2000) as i64;
    let base = chrono::Utc.with_ymd_and_hms(2000, 1, 1, 12, 0, 0).unwrap()
        + chrono::Duration::days(days_since_j2000);
    base.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}
