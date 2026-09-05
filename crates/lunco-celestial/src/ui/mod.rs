//! Celestial UI panels — time control and celestial body browser.

mod moon_map;

pub use moon_map::{
    geodetic_to_map_uv, normalize_longitude, populate_moon_map_view, MoonMapLocation, MoonMapPanel,
    MoonMapUiPlugin, MoonMapView, MOON_MAP_PANEL_ID,
};

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{
    icon_text_button, Panel, PanelCtx, PanelId, PanelSlot, UiIcon, WorkbenchAppExt,
};

use crate::commands::TeleportToSurface;
use lunco_core::CelestialBody;
use lunco_time::{
    realtime_rate_label, SetTimeTransport, TimeTransport, TransportMode, WorldTime,
    REALTIME_RATE_OPTIONS,
};

/// Celestial time control panel.
pub struct CelestialTimePanel;

impl Panel for CelestialTimePanel {
    fn id(&self) -> PanelId {
        PanelId("celestial_time")
    }
    fn title(&self) -> String {
        "Time Control".into()
    }
    fn menu_group(&self) -> lunco_workbench::PanelMenuGroup {
        lunco_workbench::PanelMenuGroup::Scene
    }
    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Bottom
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        if let Some(theme) = ctx.resource::<lunco_theme::Theme>() {
            let raised = theme.tokens.surface_raised;
            ui.style_mut().visuals.widgets.inactive.weak_bg_fill = raised;
            ui.style_mut().visuals.widgets.inactive.bg_fill = raised;
        }

        ui.heading("Epoch & UTC Time");
        // Snapshot the time state up front. Epoch comes from the derived
        // `WorldTime`; play/rate from the `TimeTransport` authority (doc 19).
        let epoch = ctx.resource::<WorldTime>().map(|w| w.epoch_jd);
        let transport = ctx
            .resource::<TimeTransport>()
            .map(|t| (matches!(t.mode, TransportMode::Paused), t.rate));

        if let Some(epoch) = epoch {
            ui.label(format!("JD: {:.4}", epoch));
            ui.label(format!("UTC: {}", jd_to_utc_string(epoch)));
        }

        let (paused, speed) = transport.unwrap_or((false, 1.0));

        ui.horizontal(|ui| {
            let (icon, label, tooltip) = if paused {
                (UiIcon::Play, "Play", "Resume simulation")
            } else {
                (UiIcon::Pause, "Pause", "Pause simulation")
            };
            if icon_text_button(ui, icon, label, tooltip).clicked() {
                ctx.trigger(SetTimeTransport {
                    playing: Some(paused),
                    rate: None,
                });
            }
        });
        ui.label(egui::RichText::new("Physics realtime").weak().small());
        ui.horizontal_wrapped(|ui| {
            for &m in REALTIME_RATE_OPTIONS {
                if ui
                    .selectable_label(speed == m, realtime_rate_label(m))
                    .clicked()
                {
                    ctx.trigger(SetTimeTransport {
                        playing: Some(true),
                        rate: Some(m),
                    });
                }
            }
        });
    }
}

/// Celestial bodies browser panel.
pub struct CelestialBodiesPanel;

impl Panel for CelestialBodiesPanel {
    fn id(&self) -> PanelId {
        PanelId("celestial_bodies")
    }
    fn title(&self) -> String {
        "Celestial Bodies".into()
    }
    fn menu_group(&self) -> lunco_workbench::PanelMenuGroup {
        lunco_workbench::PanelMenuGroup::Scene
    }
    fn default_slot(&self) -> PanelSlot {
        PanelSlot::SideBrowser
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        if let Some(theme) = ctx.resource::<lunco_theme::Theme>() {
            let raised = theme.tokens.surface_raised;
            ui.style_mut().visuals.widgets.inactive.weak_bg_fill = raised;
            ui.style_mut().visuals.widgets.inactive.bg_fill = raised;
        }

        // Read the precomputed body list (built by
        // `populate_celestial_bodies_view`, change-gated). Collect the
        // teleport intent during paint; emit it after the `view` borrow
        // ends through the typed command boundary.
        let mut teleport: Option<(Entity, Entity)> = None;
        if let Some(view) = ctx.resource::<CelestialBodiesView>() {
            let avatar = view.avatar;
            for row in &view.bodies {
                ui.horizontal(|ui| {
                    ui.label(format!("{} ({})", row.name, row.radius_label));
                    if ui.small_button("Surface").clicked() {
                        if let Some(av) = avatar {
                            teleport = Some((av, row.entity));
                        }
                    }
                });
            }
        }

        if let Some((target, body_entity)) = teleport {
            ctx.trigger(TeleportToSurface {
                target,
                body_entity,
            });
        }
    }
}

/// Change-gated view-model for the celestial body browser (WP-8).
///
/// `CelestialBodiesPanel` used to run two world scans per frame (an
/// `LocalAvatar` lookup and a `(Entity, &Name, &CelestialBody)` walk). Neither
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
    /// The body entity passed to `TeleportToSurface`.
    entity: Entity,
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
    changed: Query<
        (),
        (
            With<CelestialBody>,
            Or<(Changed<CelestialBody>, Changed<Name>)>,
        ),
    >,
    mut removed: RemovedComponents<CelestialBody>,
    avatar: Query<Entity, With<lunco_core::LocalAvatar>>,
) {
    let avatar_ent = avatar.single().ok();
    let dirty = !changed.is_empty() || removed.read().next().is_some() || view.avatar != avatar_ent;
    if !dirty {
        return;
    }

    view.avatar = avatar_ent;
    view.bodies = bodies
        .iter()
        .map(|(e, n, body)| CelestialBodyRow {
            entity: e,
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

/// Plugin that registers celestial UI panels, including the active lunar map.
pub struct CelestialUiPlugin;

impl Plugin for CelestialUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CelestialBodiesView>();
        app.add_systems(Update, populate_celestial_bodies_view);
        app.add_plugins(MoonMapUiPlugin);
        app.register_panel(CelestialTimePanel);
        app.register_panel(CelestialBodiesPanel);
    }
}
