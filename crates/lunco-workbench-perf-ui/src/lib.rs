//! Reusable performance diagnostics and status-bar HUD state.
//!
//! The capability is separate from the concrete Workbench shell so physics
//! bridges, status renderers, and other hosts can consume the same resources
//! and command without depending on the shell implementation.

//! Performance HUD for the status bar.
//!
//! Off by default. Persisted via `lunco-settings` (one shared
//! shared LunCoSim settings file so the user's choice survives restarts.
//! Three ways to flip it:
//!
//! - **Settings menu** — `Settings ▸ Performance HUD` checkbox.
//! - **Typed command** — [`TogglePerfHud`] over the API/script bus.
//! - **Direct mutation** — write to [`PerfHudSettings::enabled`].
//!
//! Live samples (`fps`, `frame_ms`, `physics_ms`) live on a separate
//! [`PerfStats`] resource — those don't belong in persistable
//! settings. The status bar reads from `PerfStats` for the numbers
//! and from `PerfHudSettings.enabled` for visibility.
//!
//! Workbench itself stays physics-agnostic: `physics_ms` is a plain
//! `Option<f32>` that another crate (e.g. `lunco-luncosim-edit-ui`)
//! populates when avian is in the build.

use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::prelude::*;
use lunco_core::{Command, on_command, register_commands};
use lunco_exposure_core::{EngineExposures, ExposureValue};
use lunco_settings::{AppSettingsExt, SettingsSection};
use serde::{Deserialize, Serialize};

/// Persisted user preference for the perf HUD. Stored under the
/// `"perf_hud"` key of `settings.json`.
#[derive(Resource, Serialize, Deserialize, Default, Clone, Copy, PartialEq, Debug)]
pub struct PerfHudSettings {
    /// Whether the HUD shows in the status bar.
    pub enabled: bool,
}

impl SettingsSection for PerfHudSettings {
    const KEY: &'static str = "perf_hud";
}

/// Live, per-frame perf samples. Not persisted — these are reset
/// when the HUD is disabled and resampled while it's on.
#[derive(Resource, Default, Debug, Clone)]
pub struct PerfStats {
    /// Smoothed FPS from Bevy's `FrameTimeDiagnosticsPlugin`.
    pub fps: f32,
    /// Smoothed frame time in milliseconds.
    pub frame_ms: f32,
    /// Wall-clock cost of the avian physics step, ms. `None` when no
    /// physics-aware plugin is publishing.
    pub physics_ms: Option<f32>,
}

/// Flip the perf HUD on/off. Persisted via `lunco-settings`.
#[Command(default)]
pub struct TogglePerfHud {
    /// `true` enables the HUD; `false` hides it.
    pub enabled: bool,
}

#[on_command(TogglePerfHud)]
fn on_toggle_perf_hud(trigger: On<TogglePerfHud>, mut settings: ResMut<PerfHudSettings>) {
    let new = trigger.event().enabled;
    if settings.enabled != new {
        settings.enabled = new;
    }
}

register_commands!(on_toggle_perf_hud,);

fn exposure_number(exposures: Option<&EngineExposures>, property: &str) -> Option<f64> {
    exposures?
        .surfaces
        .get("engine-health")?
        .properties
        .get(property)
        .and_then(|value| match value {
            ExposureValue::Number(value) => Some(*value),
            _ => None,
        })
}

/// Copy the common engine-health publication into the UI-facing view model.
///
/// The UI owns this small adapter, but it is not a second producer: the
/// runtime publishes typed snapshots once, the exposure layer commits the
/// `engine-health` capability, and every HUD surface reads that boundary.
fn sync_perf_stats(
    exposures: Option<Res<EngineExposures>>,
    settings: Res<PerfHudSettings>,
    mut stats: ResMut<PerfStats>,
) {
    if !settings.enabled {
        if stats.fps != 0.0 || stats.frame_ms != 0.0 || stats.physics_ms.is_some() {
            *stats = PerfStats::default();
        }
        return;
    }
    stats.fps = exposure_number(exposures.as_deref(), "fps").unwrap_or_default() as f32;
    stats.frame_ms =
        exposure_number(exposures.as_deref(), "frame_time_ms").unwrap_or_default() as f32;
    stats.physics_ms =
        exposure_number(exposures.as_deref(), "physics_step_ms").map(|value| value as f32);
}

/// Push the perf HUD's row into the workbench Settings menu.
fn register_settings_submenu(world: &mut World) {
    use bevy_egui::egui;
    let Some(mut menus) = world.get_resource_mut::<lunco_workbench_core::WorkbenchMenuRegistry>()
    else {
        return;
    };
    menus.register_settings_submenu("Performance", |ui, ctx| {
        ui.label(egui::RichText::new("Performance HUD").weak().small());
        let Some(mut settings) = ctx.resource::<PerfHudSettings>().copied() else {
            return;
        };
        let original = settings;
        ui.checkbox(&mut settings.enabled, "Show FPS / frame time in status bar")
            .on_hover_text(
                "Bottom-right of the status bar shows live FPS, frame \
                 time, and physics step time when an avian-aware crate \
                 is loaded. Persisted to the shared LunCoSim settings file.",
            );
        if settings != original {
            ctx.set_resource(settings);
        }
    });
}

/// Adds [`PerfStats`] (live view samples), [`PerfHudSettings`] (persisted
/// pref via `lunco-settings`), the [`TogglePerfHud`] command, Bevy's
/// frame-time diagnostics, and the Settings-menu row. Idempotent.
///
/// `FrameTimeDiagnosticsPlugin` is registered unconditionally so toggling the
/// HUD works immediately. The HUD does not independently sample or publish
/// diagnostics; it only copies the shared `engine-health` exposure.
pub struct PerfHudPlugin;

impl Plugin for PerfHudPlugin {
    fn build(&self, app: &mut App) {
        app.register_settings_section::<PerfHudSettings>();
        app.init_resource::<PerfStats>();
        // Frame diagnostics are the raw source retained by Bevy. The runtime
        // snapshot publisher consumes them once for all application surfaces.
        if !app.is_plugin_added::<FrameTimeDiagnosticsPlugin>() {
            // Deep enough for the sparkline — this IS the sparkline's buffer now.
            app.add_plugins(FrameTimeDiagnosticsPlugin::new(
                lunco_core_runtime::ENGINE_HEALTH_HISTORY_LEN,
            ));
        }
        app.add_systems(
            PostUpdate,
            sync_perf_stats.in_set(lunco_core::RuntimeCycleSet::Ui),
        );
        app.add_systems(Startup, register_settings_submenu);
        register_all_commands(app);
    }
}
