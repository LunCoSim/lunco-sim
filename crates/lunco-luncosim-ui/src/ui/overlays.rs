//! Which floating overlays the viewport shows — a persisted user preference.
//!
//! Native HUD preferences live at their existing owners. View → HUDs is the
//! single menu for visibility; camera and time menus retain their domain controls.

use bevy::prelude::*;
use lunco_capture::camera_feed::{CameraFeeds, CreateCameraFeed, MAX_CAMERA_FEEDS};
use lunco_input_ui::InputOverlaySettings;
use lunco_settings::{AppSettingsExt, SettingsSection};
use lunco_workbench_core::{
    HudVisibility, MenuCtx, SetAllHudVisibility, WorkbenchMenuRegistry, WorkbenchSnapshot,
};
use lunco_workbench_perf_ui::PerfHudSettings;
use lunco_workbench_state::RuntimeSurfaceLayouts;
use serde::{Deserialize, Serialize};

/// Persisted visibility of the optional viewport overlays. Stored under the
/// `"overlays"` key of `settings.json`.
///
/// `Default` is all-off: a fresh install shows the terrain and nothing on top of
/// it. Both fields are opt-IN, the same rule the celestial subsystem and the
/// trajectory lines already follow — content and chrome appear because something
/// asked for them, never because a default said yes.
#[derive(Resource, Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub(crate) struct OverlaySettings {
    /// Persisted schema marker. A missing marker represents settings written
    /// before the HUD's opt-in default was made explicit and is migrated below.
    #[serde(default = "overlay_settings_schema_version")]
    schema_version: u8,
    #[serde(default)]
    /// The sky-time pill (top-left): the shared CelestialTime epoch.
    pub sky_clock: bool,
    #[serde(default)]
    /// The view-mode switcher pill (top-centre): Surface / Moon / Earth, which
    /// doubles as the readout of which body the camera is focused on.
    pub view_switcher: bool,
}

const OVERLAY_SETTINGS_SCHEMA_VERSION: u8 = 1;

fn overlay_settings_schema_version() -> u8 {
    // A missing marker identifies settings written before versioned overlay
    // preferences existed. Fresh defaults set the current version explicitly.
    0
}

impl Default for OverlaySettings {
    fn default() -> Self {
        Self {
            schema_version: OVERLAY_SETTINGS_SCHEMA_VERSION,
            sky_clock: false,
            view_switcher: false,
        }
    }
}

impl SettingsSection for OverlaySettings {
    const KEY: &'static str = "overlays";

    fn migrate_persisted(&mut self) {
        if self.schema_version < OVERLAY_SETTINGS_SCHEMA_VERSION {
            self.sky_clock = false;
            self.schema_version = OVERLAY_SETTINGS_SCHEMA_VERSION;
        }
    }
}

/// `run_if` for the sky-clock overlay.
pub(crate) fn sky_clock_visible(settings: Option<Res<OverlaySettings>>) -> bool {
    settings.is_some_and(|s| s.sky_clock)
}

/// Contribute celestial clock controls to the Time menu. Visibility belongs
/// to View → HUDs.
///
/// Registered at `Startup`; a no-op when the workbench layout is absent (headless
/// runs, `luncosim test`), which is why it takes `&mut World` and bails rather than
/// requiring the resource.
pub(crate) fn register_time_menu(world: &mut World) {
    let Some(mut menus) = world.get_resource_mut::<WorkbenchMenuRegistry>() else {
        return;
    };
    menus.register_time_menu(|ui, ctx| {
        super::celestial_time::sky_clock_menu_ui(ui, ctx);
    });
}

const CELESTIAL_VIEW_SURFACE_ID: &str = "celestial-view";

fn in_view(ctx: &MenuCtx<'_>) -> bool {
    ctx.resource::<WorkbenchSnapshot>()
        .and_then(|s| s.active_perspective())
        .is_some_and(|id| id.as_str() == lunco_workbench_core::VIEW_PERSPECTIVE_ID)
}

fn visibility_row(ui: &mut bevy_egui::egui::Ui, value: &mut bool, available: bool, label: &str) {
    if let Some(enabled) = (HudVisibility {
        enabled: *value,
        available,
    })
    .menu_row(ui, label)
    {
        *value = enabled;
    }
}

/// Register the native owners in the common HUD menu.
fn register_hud_settings_menu(world: &mut World) {
    let Some(mut menus) = world.get_resource_mut::<WorkbenchMenuRegistry>() else {
        return;
    };
    menus.register_hud_menu(lunco_workbench_core::HudMenuGroup::Native, |ui, ctx| {
        use bevy_egui::egui;

        ui.label(
            egui::RichText::new("User-controlled HUDs")
                .strong()
                .text_style(lunco_theme::TypographyRole::Label.text_style()),
        );

        let Some(mut overlays) = ctx.resource::<OverlaySettings>().copied() else {
            return;
        };
        let original_overlays = overlays;
        let in_view = in_view(ctx);
        lunco_viz::graph_hud_menu_ui(ui, ctx, in_view);
        super::camera_feed::camera_hud_menu_ui(ui, ctx, in_view);

        if let Some(feeds) = ctx.resource::<CameraFeeds>() {
            let camera_view_count = feeds.instances.len();
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Camera views").strong());
                ui.label(
                    egui::RichText::new(format!("{camera_view_count}/{MAX_CAMERA_FEEDS}"))
                        .weak()
                        .small(),
                );
                let add_view = ui.add_enabled(
                    camera_view_count < MAX_CAMERA_FEEDS,
                    egui::Button::new("+ Add camera view"),
                );
                if add_view
                    .on_hover_text(if camera_view_count < MAX_CAMERA_FEEDS {
                        "Open another independent camera view. Choose its camera from the view toolbar."
                    } else {
                        "All camera views are in use. Close a view before adding another."
                    })
                    .clicked()
                {
                    ctx.trigger(CreateCameraFeed {});
                }
            });
        } else {
            ui.label(egui::RichText::new("Camera views unavailable").weak());
        }

        visibility_row(ui, &mut overlays.sky_clock, in_view, "Time HUD");
        visibility_row(ui, &mut overlays.view_switcher, in_view, "Surface view switcher");
        if overlays != original_overlays {
            ctx.set_resource(overlays);
        }

        let Some(mut surface_layouts) = ctx.resource::<RuntimeSurfaceLayouts>().cloned() else {
            return;
        };
        let has_surface_override = surface_layouts.get(CELESTIAL_VIEW_SURFACE_ID).is_some();
        let mut reset_surface_position = false;
        ui.horizontal(|ui| {
            ui.label("Surface view switcher position");
            if ui
                .add_enabled(has_surface_override, egui::Button::new("Reset position"))
                .on_hover_text("Restore the authored top-centre position for this Twin.")
                .clicked()
            {
                reset_surface_position = surface_layouts.reset(CELESTIAL_VIEW_SURFACE_ID);
            }
        });
        if reset_surface_position {
            ctx.set_resource(surface_layouts);
        }
        ui.label(
            egui::RichText::new(if has_surface_override {
                "Custom position"
            } else {
                "Authored default: top-centre"
            })
            .weak()
            .small(),
        );

        let Some(mut performance) = ctx.resource::<PerfHudSettings>().copied() else {
            return;
        };
        let original_performance = performance;
        visibility_row(ui, &mut performance.enabled, true, "Performance HUD");
        if performance != original_performance {
            ctx.set_resource(performance);
        }

        let Some(mut input) = ctx.resource::<InputOverlaySettings>().copied() else {
            return;
        };
        let original_input = input;
        visibility_row(ui, &mut input.enabled, true, "Input HUD");
        if input != original_input {
            ctx.set_resource(input);
        }

    });
}

fn set_all_native_hud_visibility(
    event: On<SetAllHudVisibility>,
    mut overlays: ResMut<OverlaySettings>,
    mut performance: Option<ResMut<PerfHudSettings>>,
    mut input: Option<ResMut<InputOverlaySettings>>,
) {
    let enabled = event.0;
    if overlays.sky_clock != enabled {
        overlays.sky_clock = enabled;
    }
    if overlays.view_switcher != enabled {
        overlays.view_switcher = enabled;
    }
    if let Some(performance) = performance.as_mut() {
        if performance.enabled != enabled {
            performance.enabled = enabled;
        }
    }
    if let Some(input) = input.as_mut() {
        if input.enabled != enabled {
            input.enabled = enabled;
        }
    }
}

/// Registers [`OverlaySettings`] (persisted) and its menu rows.
pub(crate) fn plugin(app: &mut App) {
    app.register_settings_section::<OverlaySettings>();
    app.add_systems(Startup, (register_time_menu, register_hud_settings_menu))
        .add_observer(set_all_native_hud_visibility);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_hides_the_sky_clock() {
        assert!(!OverlaySettings::default().sky_clock);
    }

    #[test]
    fn legacy_persisted_sky_clock_is_migrated_off_once() {
        let mut settings: OverlaySettings = serde_json::from_value(serde_json::json!({
            "sky_clock": true,
            "view_switcher": false,
        }))
        .expect("legacy overlay settings");
        settings.migrate_persisted();
        assert!(!settings.sky_clock);
        assert_eq!(settings.schema_version, OVERLAY_SETTINGS_SCHEMA_VERSION);
    }

    #[test]
    fn migrated_opt_in_remains_persisted() {
        let mut settings = OverlaySettings {
            schema_version: OVERLAY_SETTINGS_SCHEMA_VERSION,
            sky_clock: true,
            view_switcher: false,
        };
        settings.migrate_persisted();
        assert!(settings.sky_clock);
    }
}
