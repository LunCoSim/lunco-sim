//! Mission Control panel — single unified panel for time, bodies, spacecraft, rovers, and actions.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot};

use lunco_core::{Avatar, Spacecraft};
use lunco_fsw::FlightSoftware;
use lunco_time::{TimeTransport, TransportMode, WorldTime};
use lunco_celestial::{CelestialBody, TeleportToSurface, LeaveSurface};
use lunco_avatar::{PossessVessel, ReleaseVessel, FocusTarget};

/// Mission Control panel — everything in one place.
pub struct MissionControl;

impl Panel for MissionControl {
    fn id(&self) -> PanelId { PanelId("mission_control") }
    fn title(&self) -> String { "Mission Control".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::RightInspector }
    fn transparent_background(&self) -> bool { true }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        // ── Snapshot all derived/resource state up front so every `ctx`
        //    read borrow ends before any `ctx.defer` below. The panel reads
        //    the change-gated `MissionControlView` (no per-frame scans) plus
        //    O(1) networking resources.
        let theme = ctx.resource::<lunco_theme::Theme>().cloned();

        let avatar_ent;
        let clock_state;
        let bodies: Vec<(Entity, String, String)>;
        let spacecraft: Vec<(Entity, String)>;
        let rovers_display: Vec<(Entity, String, bool, bool, Option<u64>)>;
        let on_surface;
        let gravity_body;
        let networked;
        let is_host;
        let policy;
        {
            let view = ctx.resource::<MissionControlView>();
            avatar_ent = view.and_then(|v| v.avatar);
            bodies = view
                .map(|v| {
                    v.bodies
                        .iter()
                        .map(|r| (r.entity, r.name.clone(), r.label.clone()))
                        .collect()
                })
                .unwrap_or_default();
            spacecraft = view
                .map(|v| v.spacecraft.iter().map(|r| (r.entity, r.name.clone())).collect())
                .unwrap_or_default();
            on_surface = view.map(|v| v.on_surface).unwrap_or(false);
            gravity_body = view.and_then(|v| v.gravity_body);

            // Epoch from the derived `WorldTime`; play/rate from the
            // `TimeTransport` authority (doc 19 — the `CelestialClock` middleman is
            // gone). Both are inserted together by `TimePlugin`, so the tuple is
            // `Some` iff the spine is present.
            clock_state = ctx.resource::<WorldTime>().and_then(|w| {
                ctx.resource::<TimeTransport>()
                    .map(|t| (w.epoch_jd, matches!(t.mode, TransportMode::Paused), t.rate))
            });

            // Networking context, read from the always-on substrate. In
            // single-player (Standalone, empty registry) `networked` is false
            // so the ownership UI stays hidden and rovers behave as before.
            let local_session = ctx.resource::<lunco_core::LocalSession>().map(|l| l.0);
            let role = ctx.resource::<lunco_core::NetworkRole>();
            networked = role.map(|r| r.is_networked()).unwrap_or(false);
            is_host = role.map(|r| r.is_host()).unwrap_or(false);
            let reg = ctx.resource::<lunco_core::SessionRegistry>();
            policy = reg.map(|r| r.policy()).unwrap_or_default();

            rovers_display = view
                .map(|v| {
                    v.rovers
                        .iter()
                        .map(|r| {
                            let owner = r.gid.and_then(|g| reg.and_then(|reg| reg.owner_of(g)));
                            let mine = matches!(
                                (owner, local_session),
                                (Some(o), Some(l)) if o == l
                            );
                            let taken_by_other = owner.is_some() && !mine;
                            (r.entity, r.name.clone(), mine, taken_by_other, owner.map(|s| s.0))
                        })
                        .collect()
                })
                .unwrap_or_default();
        }

        if let Some(t) = &theme {
            ui.style_mut().visuals = t.to_visuals();
        }

        // ── Intents collected during paint, emitted via `ctx.defer` after. ──
        let mut focus: Option<Entity> = None;
        let mut teleport_body_bits: Option<u64> = None;
        let mut possess: Option<Entity> = None;
        let mut release = false;
        let mut leave_surface = false;
        let mut toggle_pause = false;
        let mut set_speed: Option<f64> = None;
        let mut set_policy: Option<lunco_core::PossessionPolicy> = None;

        let frame = egui::Frame::new()
            .fill(theme.as_ref().map(|t| t.colors.mantle).unwrap_or(egui::Color32::TRANSPARENT))
            .inner_margin(8.0)
            .corner_radius(4);

        frame.show(ui, |ui| {
            // ── Time Control ──
            ui.heading("Time Control");
            if let Some((epoch, _, _)) = clock_state {
                ui.label(format!("JD: {:.4}", epoch));
                ui.label(format!("UTC: {}", jd_to_utc_string(epoch)));
            }
            if let Some((_, paused, speed)) = clock_state {
                ui.horizontal(|ui| {
                    if ui.button(if paused { "▶ Play" } else { "⏸ Pause" }).clicked() {
                        toggle_pause = true;
                    }
                });
                // Two bands, because they do PHYSICALLY DIFFERENT THINGS and the
                // difference used to be invisible. At or below MAX_REALTIME_RATE the
                // rate multiplies the number of fixed steps per frame, so bodies
                // genuinely integrate faster (a rover really drives 4× faster). Above
                // it, `advance_clock` selects `TimeRegime::KinematicWarp` and returns
                // relative_speed 0 — the tick FREEZES and only the epoch (sky, orbits)
                // advances. The old row ran 1x → 10x, so the first click past realtime
                // silently stopped the rover dead while the sky sped up.
                egui::Grid::new("time_multipliers")
                    .num_columns(4)
                    .spacing([4.0, 4.0])
                    .show(ui, |ui| {
                        for (i, &m) in [1.0, 2.0, 4.0, 8.0].iter().enumerate() {
                            if ui
                                .selectable_label(speed == m, format!("{}x", m))
                                .on_hover_text("Physics runs at this rate")
                                .clicked()
                            {
                                set_speed = Some(m);
                            }
                            if (i + 1) % 4 == 0 {
                                ui.end_row();
                            }
                        }
                    });
                ui.label(
                    egui::RichText::new("sky only — physics frozen")
                        .weak()
                        .size(10.0),
                );
                egui::Grid::new("time_multipliers_warp")
                    .num_columns(4)
                    .spacing([4.0, 4.0])
                    .show(ui, |ui| {
                        for (i, &m) in [100.0, 1000.0, 10000.0, 100000.0].iter().enumerate() {
                            if ui
                                .selectable_label(speed == m, format!("{}x", m))
                                .on_hover_text(
                                    "Kinematic warp: the sim tick freezes. \
                                     Bodies do not move; only the epoch advances.",
                                )
                                .clicked()
                            {
                                set_speed = Some(m);
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
                for (entity, name, radius) in &bodies {
                    ui.horizontal(|ui| {
                        ui.label(format!("{} ({})", name, radius));
                        if ui.small_button("Focus").clicked() {
                            focus = Some(*entity);
                        }
                        if ui.small_button("🌕 Surface").clicked() {
                            teleport_body_bits = Some(entity.to_bits());
                        }
                    });
                }
            });

            // ── Spacecraft ──
            ui.collapsing("Spacecraft", |ui| {
                for (entity, name) in &spacecraft {
                    ui.horizontal(|ui| {
                        ui.label(name);
                        if ui.small_button("Focus").clicked() {
                            focus = Some(*entity);
                        }
                    });
                }
            });

            // ── Rovers ──
            ui.collapsing("Rovers", |ui| {
                // Possession policy selector (host-authoritative — only the
                // host's choice governs `claim`). Default `Exclusive` = "one each".
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
                        set_policy = Some(chosen);
                    }
                }

                for (entity, name, mine, taken_by_other, owner_sess) in &rovers_display {
                    ui.horizontal(|ui| {
                        // Ownership chip (only meaningful with the wire live).
                        if networked {
                            if *mine {
                                let mine_dot = theme
                                    .as_ref()
                                    .map(|t| t.tokens.success)
                                    .unwrap_or(egui::Color32::from_rgb(0x4c, 0xff, 0x88));
                                ui.colored_label(mine_dot, "●")
                                    .on_hover_text("You control this rover");
                            } else if *taken_by_other {
                                // TODO(theme): migrate to lunco-theme once the token set covers this.
                                // "Owned by another session" — unavailable, not broken. Neither
                                // `error` (nothing failed) nor `warning` (yellow, wrong hue) fits.
                                ui.colored_label(egui::Color32::from_rgb(0xff, 0x6b, 0x4c), "🔒")
                                    .on_hover_text(format!(
                                        "Controlled by session {}",
                                        owner_sess.unwrap_or(0)
                                    ));
                            } else {
                                ui.weak("○").on_hover_text("Free to possess");
                            }
                        }
                        ui.label(name);
                        if ui.small_button("Focus").clicked() {
                            focus = Some(*entity);
                        }
                        if *mine {
                            if ui.small_button("🚪 Release").clicked() {
                                release = true;
                            }
                        } else {
                            // Under `Exclusive` a rover held by another is locked;
                            // under `LastWins` anyone can take it.
                            let locked = *taken_by_other
                                && matches!(policy, lunco_core::PossessionPolicy::Exclusive);
                            let resp =
                                ui.add_enabled(!locked, egui::Button::new("🚗 Possess").small());
                            if locked {
                                resp.on_disabled_hover_text(
                                    "Controlled by another player (One-each policy)",
                                );
                            } else if resp.clicked() {
                                possess = Some(*entity);
                            }
                        }
                    });
                }
            });

            // ── Quick Actions ──
            ui.separator();
            ui.heading("Quick Actions");
            if avatar_ent.is_some() {
                if ui.button("🚀 Release (Free Fly)").clicked() {
                    release = true;
                }
                // Return to Orbit — show when avatar is in surface mode.
                if on_surface && ui.button("🏠 Return to Orbit").clicked() {
                    leave_surface = true;
                }
            }

            ui.separator();
            ui.label("Double-click entities in Inspector to focus.");
            ui.label("WASD: move  |  QE: Up/Down");
            ui.label("Right-Click: rotate  |  SPACE: pause");
        });

        // ── Emit collected intent after paint (read borrows released). ──
        if let Some(av) = avatar_ent {
            if let Some(target) = focus {
                ctx.defer(move |world| {
                    world.trigger(FocusTarget { avatar: Some(av), target });
                });
            }
            if let Some(body_entity) = teleport_body_bits {
                ctx.defer(move |world| {
                    world.trigger(TeleportToSurface { target: av, body_entity });
                });
            }
            if let Some(target) = possess {
                ctx.defer(move |world| {
                    world.trigger(PossessVessel { avatar: Some(av), target });
                });
            }
            if release {
                ctx.defer(move |world| {
                    world.trigger(ReleaseVessel { target: av });
                });
            }
            if leave_surface && gravity_body.is_some() {
                ctx.defer(move |world| {
                    world.trigger(LeaveSurface { target: av });
                });
            }
        }

        if toggle_pause {
            let cur = clock_state.map(|(_, p, _)| p).unwrap_or(false);
            ctx.defer(move |world| {
                if let Some(mut t) = world.get_resource_mut::<TimeTransport>() {
                    t.mode = if cur { TransportMode::Playing } else { TransportMode::Paused };
                }
            });
        }
        if let Some(m) = set_speed {
            ctx.defer(move |world| {
                if let Some(mut t) = world.get_resource_mut::<TimeTransport>() {
                    t.rate = m;
                }
            });
        }
        if let Some(p) = set_policy {
            ctx.defer(move |world| {
                if let Some(mut reg) = world.get_resource_mut::<lunco_core::SessionRegistry>() {
                    reg.set_policy(p);
                }
            });
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// View-model (WP-8) — change-gated derived data for the panel.
// ─────────────────────────────────────────────────────────────────────

/// Change-gated view-model for [`MissionControl`].
///
/// The panel used to run ~5 world scans per frame (avatar, bodies,
/// spacecraft, rovers, surface-camera). None depend on per-frame UI state,
/// so [`populate_mission_control_view`] flattens them into this resource —
/// rebuilt only when the relevant components change / despawn or the avatar,
/// surface mode, or gravity body change — and the panel reads it via
/// `ctx.resource`. Ownership (which session holds each rover) stays in the
/// panel as O(1) `SessionRegistry` lookups since it changes per network tick.
#[derive(Resource, Default)]
pub struct MissionControlView {
    avatar: Option<Entity>,
    bodies: Vec<EntityRow>,
    spacecraft: Vec<EntityRow>,
    rovers: Vec<RoverRow>,
    on_surface: bool,
    gravity_body: Option<Entity>,
}

/// A focusable entity row (body or spacecraft).
struct EntityRow {
    entity: Entity,
    name: String,
    /// Pre-formatted secondary label (e.g. radius); empty when unused.
    label: String,
}

/// A rover row. `gid` is the stable global id used for ownership lookup.
struct RoverRow {
    entity: Entity,
    name: String,
    gid: Option<u64>,
}

/// Producer for [`MissionControlView`]. Steady state is a handful of
/// `is_empty`/scalar checks; the scans only run on a relevant change.
pub fn populate_mission_control_view(
    mut view: ResMut<MissionControlView>,
    avatar: Query<Entity, With<Avatar>>,
    bodies: Query<(Entity, &Name, &CelestialBody)>,
    spacecraft: Query<(Entity, &Name), With<Spacecraft>>,
    // The local avatar now carries a `FlightSoftware` command surface too (it's a
    // controllable), so exclude it from the *rover* list.
    rovers: Query<(Entity, &Name, Option<&lunco_core::GlobalEntityId>), (With<FlightSoftware>, Without<Avatar>)>,
    surface: Query<(), With<lunco_avatar::SurfaceCamera>>,
    gravity: Option<Res<lunco_celestial::LocalGravityField>>,
    changed: Query<
        (),
        Or<(
            Changed<CelestialBody>,
            Changed<Spacecraft>,
            Changed<FlightSoftware>,
            Changed<Name>,
            Changed<lunco_core::GlobalEntityId>,
        )>,
    >,
    mut removed_body: RemovedComponents<CelestialBody>,
    mut removed_sc: RemovedComponents<Spacecraft>,
    mut removed_rover: RemovedComponents<FlightSoftware>,
) {
    let avatar_ent = avatar.iter().next();
    let on_surface = !surface.is_empty();
    let gravity_body = gravity.and_then(|g| g.body_entity);

    let dirty = !changed.is_empty()
        || removed_body.read().next().is_some()
        || removed_sc.read().next().is_some()
        || removed_rover.read().next().is_some()
        || view.avatar != avatar_ent
        || view.on_surface != on_surface
        || view.gravity_body != gravity_body;
    if !dirty {
        return;
    }

    view.avatar = avatar_ent;
    view.on_surface = on_surface;
    view.gravity_body = gravity_body;
    view.bodies = bodies
        .iter()
        .map(|(e, n, b)| EntityRow {
            entity: e,
            name: n.as_str().to_string(),
            label: format!("{:.0} km", b.radius_m / 1000.0),
        })
        .collect();
    view.spacecraft = spacecraft
        .iter()
        .map(|(e, n)| EntityRow {
            entity: e,
            name: n.as_str().to_string(),
            label: String::new(),
        })
        .collect();
    view.rovers = rovers
        .iter()
        .map(|(e, n, g)| RoverRow {
            entity: e,
            name: n.as_str().to_string(),
            gid: g.map(|g| g.get()),
        })
        .collect();
}

/// Format a TDB epoch (Julian Date) as a UTC string via the spine — all the
/// time-scale nuance (TDB→TT→TAI→UTC, leap seconds) lives in `lunco-time`
/// (doc 19 — T3), so this UI never re-derives JD↔UTC. The old local version
/// treated the master epoch as UTC (≈69 s early) anchored at J2000.
fn jd_to_utc_string(jd: f64) -> String {
    lunco_time::tdb_jd_to_utc_string(jd)
}
