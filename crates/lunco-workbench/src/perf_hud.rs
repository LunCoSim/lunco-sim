//! Performance HUD for the status bar.
//!
//! Off by default. Persisted via `lunco-settings` (one shared
//! `~/.lunco/settings.json`) so the user's choice survives restarts.
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
//! `Option<f32>` that another crate (e.g. `lunco-sandbox-edit`)
//! populates when avian is in the build.

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use lunco_core::{Command, on_command, register_commands};
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

/// How many frame-time samples to keep for the status-bar sparkline.
/// At 60 FPS that's about 4 seconds — long enough to spot a hitch
/// in your peripheral vision, short enough that the plot redraws
/// quickly when conditions change.
pub const FRAME_HISTORY_LEN: usize = 240;

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
    /// Ring buffer of recent frame times, oldest first. Used by the
    /// status-bar sparkline so spikes that the smoothed `frame_ms`
    /// number hides become visible. Capped at [`FRAME_HISTORY_LEN`].
    pub frame_history: std::collections::VecDeque<f32>,
}

impl PerfStats {
    /// `(min, max, p99)` over `frame_history`, all in ms. Returns
    /// `None` when the history is empty so callers can skip drawing.
    pub fn frame_ms_stats(&self) -> Option<(f32, f32, f32)> {
        if self.frame_history.is_empty() {
            return None;
        }
        let mut sorted: Vec<f32> = self.frame_history.iter().copied().collect();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let min = *sorted.first().unwrap();
        let max = *sorted.last().unwrap();
        let p99_idx = ((sorted.len() as f32) * 0.99) as usize;
        let p99 = sorted[p99_idx.min(sorted.len() - 1)];
        Some((min, max, p99))
    }
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

/// Read smoothed FPS / frame time from the diagnostics store into
/// [`PerfStats`]. Bails when the HUD is disabled.
fn sample_frame_time(
    diags: Res<DiagnosticsStore>,
    settings: Res<PerfHudSettings>,
    mut stats: ResMut<PerfStats>,
) {
    if !settings.enabled {
        if stats.fps != 0.0 || stats.frame_ms != 0.0 || stats.physics_ms.is_some()
            || !stats.frame_history.is_empty()
        {
            *stats = PerfStats::default();
        }
        return;
    }
    if let Some(d) = diags.get(&FrameTimeDiagnosticsPlugin::FPS) {
        if let Some(v) = d.smoothed() {
            stats.fps = v as f32;
        }
    }
    if let Some(d) = diags.get(&FrameTimeDiagnosticsPlugin::FRAME_TIME) {
        // Push the **raw** (un-smoothed) value to the history so
        // spikes survive into the sparkline; keep the smoothed value
        // for the headline number where stability is preferred.
        if let Some(raw) = d.value() {
            if stats.frame_history.len() == FRAME_HISTORY_LEN {
                stats.frame_history.pop_front();
            }
            stats.frame_history.push_back(raw as f32);
        }
        if let Some(v) = d.smoothed() {
            stats.frame_ms = v as f32;
        }
    }
}

/// Push the perf HUD's row into the workbench Settings menu.
fn register_settings_menu(world: &mut World) {
    use bevy_egui::egui;
    let Some(mut layout) = world.get_resource_mut::<crate::WorkbenchLayout>() else {
        return;
    };
    layout.register_settings(|ui, world| {
        ui.label(egui::RichText::new("Performance HUD").weak().small());
        let mut settings = world.resource_mut::<PerfHudSettings>();
        ui.checkbox(&mut settings.enabled, "Show FPS / frame time in status bar")
            .on_hover_text(
                "Bottom-right of the status bar shows live FPS, frame \
                 time, and physics step time when an avian-aware crate \
                 is loaded. Persisted to ~/.lunco/settings.json.",
            );
    });
}

/// Adds [`PerfStats`] (live samples), [`PerfHudSettings`] (persisted
/// pref via `lunco-settings`), the [`TogglePerfHud`] command, Bevy's
/// frame-time diagnostics, and the Settings-menu row. Idempotent.
///
/// `FrameTimeDiagnosticsPlugin` and the per-frame sampler are registered
/// unconditionally — they cost only a few µs/frame and the sampler
/// (`sample_frame_time`) early-bails when the HUD pref is off, so leaving
/// them on means toggling the HUD at runtime works immediately (no restart)
/// because the diagnostic data is already being collected.
pub struct PerfHudPlugin;

impl Plugin for PerfHudPlugin {
    fn build(&self, app: &mut App) {
        app.register_settings_section::<PerfHudSettings>();
        app.init_resource::<PerfStats>();
        // FrameTime diagnostics + frame sampler are always registered
        // — they're cheap (a few µs/frame), and toggling the HUD at
        // runtime needs the data to be there already. The sampler
        // bails early when the HUD is off.
        if !app.is_plugin_added::<FrameTimeDiagnosticsPlugin>() {
            app.add_plugins(FrameTimeDiagnosticsPlugin::default());
        }
        app.add_systems(Update, sample_frame_time);
        app.add_systems(Startup, register_settings_menu);
        register_all_commands(app);
    }
}

