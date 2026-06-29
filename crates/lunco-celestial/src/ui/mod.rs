//! Celestial UI panels — time control and celestial body browser.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot, WorkbenchAppExt};

use lunco_core::{Avatar, CelestialBody, CelestialClock};
use crate::commands::TeleportToSurface;

/// Celestial time control panel.
pub struct CelestialTimePanel;

impl Panel for CelestialTimePanel {
    fn id(&self) -> PanelId { PanelId("celestial_time") }
    fn title(&self) -> String { "Time Control".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::Bottom }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        if let Some(theme) = ctx.resource::<lunco_theme::Theme>() {
            let raised = theme.tokens.surface_raised;
            ui.style_mut().visuals.widgets.inactive.weak_bg_fill = raised;
            ui.style_mut().visuals.widgets.inactive.bg_fill = raised;
        }

        ui.heading("Epoch & UTC Time");
        // Snapshot the clock state up front so all reads release the
        // immutable `ctx` borrow before any `ctx.defer` below.
        let clock_state = ctx
            .resource::<CelestialClock>()
            .map(|c| (c.epoch, c.paused, c.speed_multiplier));

        if let Some((epoch, _, _)) = clock_state {
            ui.label(format!("JD: {:.4}", epoch));
            ui.label(format!("UTC: {}", jd_to_utc_string(epoch)));
        }

        let (paused, speed) = clock_state.map(|(_, p, s)| (p, s)).unwrap_or((false, 1.0));

        ui.horizontal(|ui| {
            if ui.button(if paused { "▶ Play" } else { "⏸ Pause" }).clicked() {
                ctx.defer(move |world| {
                    if let Some(mut clock) = world.get_resource_mut::<CelestialClock>() {
                        clock.paused = !paused;
                    }
                });
            }
        });
        ui.horizontal_wrapped(|ui| {
            let multipliers = [1.0, 10.0, 100.0, 1000.0, 10000.0, 100000.0, 1000000.0];
            for &m in multipliers.iter() {
                if ui.selectable_label(speed == m, format!("{}x", m)).clicked() {
                    ctx.defer(move |world| {
                        if let Some(mut clock) = world.get_resource_mut::<CelestialClock>() {
                            clock.speed_multiplier = m;
                        }
                    });
                }
            }
        });
    }
}

/// Celestial bodies browser panel.
pub struct CelestialBodiesPanel;

impl Panel for CelestialBodiesPanel {
    fn id(&self) -> PanelId { PanelId("celestial_bodies") }
    fn title(&self) -> String { "Celestial Bodies".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::SideBrowser }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        if let Some(theme) = ctx.resource::<lunco_theme::Theme>() {
            let raised = theme.tokens.surface_raised;
            ui.style_mut().visuals.widgets.inactive.weak_bg_fill = raised;
            ui.style_mut().visuals.widgets.inactive.bg_fill = raised;
        }

        // Read the precomputed body list (built by
        // `populate_celestial_bodies_view`, change-gated). Collect the
        // teleport intent during paint; emit it after the `view` borrow
        // ends so `ctx.defer` is free to take `&mut`.
        let mut teleport: Option<(Entity, u64)> = None;
        if let Some(view) = ctx.resource::<CelestialBodiesView>() {
            let avatar = view.avatar;
            for row in &view.bodies {
                ui.horizontal(|ui| {
                    ui.label(format!("{} ({})", row.name, row.radius_label));
                    if ui.small_button("🌕 Surface").clicked() {
                        if let Some(av) = avatar {
                            teleport = Some((av, row.entity_bits));
                        }
                    }
                });
            }
        }

        if let Some((target, body_entity)) = teleport {
            ctx.defer(move |world| {
                world.trigger(TeleportToSurface { target, body_entity });
            });
        }
    }
}

/// Change-gated view-model for the celestial body browser (WP-8).
///
/// `CelestialBodiesPanel` used to run two world scans per frame (an
/// `Avatar` lookup and a `(Entity, &Name, &CelestialBody)` walk). Neither
/// depends on per-frame UI state, so [`populate_celestial_bodies_view`]
/// flattens both into this resource — rebuilt only when a body's
/// `CelestialBody`/`Name` changes, a body despawns, or the avatar changes
/// — and the panel reads it via `ctx.resource`.
#[derive(Resource, Default)]
pub struct CelestialBodiesView {
    /// The avatar entity to teleport (surface button target), if any.
    avatar: Option<Entity>,
    /// One row per celestial body, in query order.
    bodies: Vec<CelestialBodyRow>,
}

/// Derived per-body row the browser renders.
struct CelestialBodyRow {
    /// Raw `Entity::to_bits()` — carried verbatim into `TeleportToSurface`.
    entity_bits: u64,
    /// Display name.
    name: String,
    /// Pre-formatted radius label, e.g. `"1737 km"`.
    radius_label: String,
}

/// Producer for [`CelestialBodiesView`]. Rebuilds the list only when a
/// body's `CelestialBody`/`Name` changes, a body is removed, or the
/// avatar entity changes — so steady state is a couple of `is_empty`
/// checks, not two full scans.
pub fn populate_celestial_bodies_view(
    mut view: ResMut<CelestialBodiesView>,
    bodies: Query<(Entity, &Name, &CelestialBody)>,
    changed: Query<(), (With<CelestialBody>, Or<(Changed<CelestialBody>, Changed<Name>)>)>,
    mut removed: RemovedComponents<CelestialBody>,
    avatar: Query<Entity, With<Avatar>>,
) {
    let avatar_ent = avatar.iter().next();
    let dirty = !changed.is_empty()
        || removed.read().next().is_some()
        || view.avatar != avatar_ent;
    if !dirty {
        return;
    }

    view.avatar = avatar_ent;
    view.bodies = bodies
        .iter()
        .map(|(e, n, body)| CelestialBodyRow {
            entity_bits: e.to_bits(),
            name: n.as_str().to_string(),
            radius_label: format!("{:.0} km", body.radius_m / 1000.0),
        })
        .collect();
}

/// Format a TDB epoch (Julian Date) as a UTC string. All time-scale nuance lives
/// in `lunco-time` (doc 19 — T3); this is a thin reuse, not a local JD↔UTC
/// re-implementation (the old one mislabelled the master epoch as UTC and
/// truncated the time-of-day to whole days).
fn jd_to_utc_string(jd: f64) -> String {
    lunco_time::tdb_jd_to_utc_string(jd)
}

/// Plugin that registers celestial UI panels.
pub struct CelestialUiPlugin;

impl Plugin for CelestialUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CelestialBodiesView>();
        app.add_systems(Update, populate_celestial_bodies_view);
        app.register_panel(CelestialTimePanel);
        app.register_panel(CelestialBodiesPanel);
    }
}
