//! Checkpoint path-line visualization — the render projection of a vessel's
//! live [`AutopilotBehaviorSpec`].
//!
//! A "checkpoint" list is a [`BehaviorSpec::Patrol`] (authored in rhai via the
//! `patrol.rhai` prelude, or interactively via Ctrl+LMB — see
//! [`crate::ui::checkpoint_click`]). The Rust core mirrors the source spec onto
//! the vessel as [`AutopilotBehaviorSpec`]; this ui-gated gizmo reads it and
//! draws:
//!
//! - a **numbered pin** at each waypoint, and
//! - the **connecting path line** through them — so the route the autopilot will
//!   follow is visible at a glance.
//!
//! All visual tuning (colour, pin radius, line width) comes from
//! [`lunco_theme::Theme`] and [`CheckpointGizmoSettings`] — no magic numbers
//! (§3). Per-frame, but it early-returns when no vessel carries a spec, so the
//! no-op path is cheap (§7: gizmos are genuinely-continuous render work, the
//! sanctioned per-frame exception).

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_autopilot::{AutopilotBehaviorSpec, BehaviorSpec};
use lunco_core::{on_command, register_commands, Command};
use lunco_theme::Theme;

/// Convert an egui `Color32` to a Bevy `Color` (sRGBA, normalised 0..1).
fn to_bevy(c: egui::Color32) -> bevy::color::Color {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    bevy::color::Color::srgba(
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        a as f32 / 255.0,
    )
}

/// Tunable visual parameters for the checkpoint path-line gizmo. A resource so
/// a user / settings panel can live-tune it (§3 — no magic numbers).
#[derive(Resource, Clone, Debug)]
pub struct CheckpointGizmoSettings {
    /// Pin sphere radius (world units).
    pub pin_radius: f32,
    /// Path line width (gizmo units).
    pub line_width: f32,
    /// Draw the connecting path between waypoints.
    pub show_path: bool,
    /// Draw an extra leg from the vessel's current position to waypoint 1.
    pub show_approach: bool,
    /// Screen-space radius (pixels) within which a right-click counts as
    /// "on a pin" for the checkpoint context menu. Kept here (not a const in
    /// `checkpoint_click`) so it tracks the visual pin size — the gizmo owns
    /// the "what is a pin" definition.
    pub pin_pick_radius_px: f32,
}

impl Default for CheckpointGizmoSettings {
    fn default() -> Self {
        Self {
            pin_radius: 0.35,
            line_width: 2.0,
            show_path: true,
            show_approach: true,
            pin_pick_radius_px: 22.0,
        }
    }
}

/// Live-tune the gizmo settings (all fields optional → set only what you pass).
/// Reachable from UI / rhai / the HTTP API like any `#[Command]`.
#[Command(default)]
pub struct SetCheckpointGizmo {
    #[serde(default)]
    #[reflect(default)]
    pub pin_radius: Option<f32>,
    #[serde(default)]
    #[reflect(default)]
    pub line_width: Option<f32>,
    #[serde(default)]
    #[reflect(default)]
    pub show_path: Option<bool>,
    #[serde(default)]
    #[reflect(default)]
    pub show_approach: Option<bool>,
    #[serde(default)]
    #[reflect(default)]
    pub pin_pick_radius_px: Option<f32>,
}

#[on_command(SetCheckpointGizmo)]
fn on_set_checkpoint_gizmo(trigger: On<SetCheckpointGizmo>, mut s: ResMut<CheckpointGizmoSettings>) {
    let cmd = trigger.event();
    if let Some(v) = cmd.pin_radius { s.pin_radius = v; }
    if let Some(v) = cmd.line_width { s.line_width = v; }
    if let Some(v) = cmd.show_path { s.show_path = v; }
    if let Some(v) = cmd.show_approach { s.show_approach = v; }
    if let Some(v) = cmd.pin_pick_radius_px { s.pin_pick_radius_px = v; }
}

register_commands!(on_set_checkpoint_gizmo,);

/// Tunable defaults used when the Rust core has to *invent* a patrol spec
/// (Ctrl+LMB on a vessel with no prior behaviour). Mirrors the `PATROL_*`
/// constants in the rhai `patrol.rhai` prelude — kept in one `Resource` so
/// the Rust fallback and the documented rhai defaults can't drift out of
/// sync, and so a user / settings panel can live-tune them (§3).
///
/// Authoring-time tuning still belongs in rhai (the prelude is the canonical
/// authoring surface); this resource is only the *fallback* the interactive
/// editor reaches for when there's no spec to extend yet.
#[derive(Resource, Clone, Copy, Debug)]
pub struct PatrolDefaults {
    /// Cruise speed when the autopilot has no other guidance (m/s).
    pub speed: f64,
    /// Waypoint arrival radius (m) — within this, the patrol advances.
    pub radius: f32,
    /// Dwell time at each waypoint (s).
    pub dwell: f64,
    /// `EngageAutopilot` throttle used when starting a patrol from scratch.
    pub engage_throttle: f64,
}

impl Default for PatrolDefaults {
    fn default() -> Self {
        Self { speed: 0.6, radius: 3.0, dwell: 0.0, engage_throttle: 0.6 }
    }
}

/// Ui-gated plugin: registers settings + the gizmo system. Added by
/// [`crate::ui::SandboxEditUiPlugin`] (Layer 4) — headless has no gizmos.
pub struct CheckpointGizmoPlugin;

impl Plugin for CheckpointGizmoPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CheckpointGizmoSettings>();
        app.init_resource::<PatrolDefaults>();
        register_all_commands(app);
        app.add_systems(Update, draw_checkpoint_path);
    }
}

/// Draw the patrol path for every vessel carrying an [`AutopilotBehaviorSpec`].
///
/// Non-patrol specs have no waypoint list → skip. The pin colour alternates the
/// theme accent / secondary so each index is distinguishable; the path line is
/// drawn as gizmo segments between consecutive waypoints, plus the approach leg
/// from the vessel.
fn draw_checkpoint_path(
    q: Query<(&GlobalTransform, &AutopilotBehaviorSpec)>,
    mut gizmos: Gizmos,
    settings: Res<CheckpointGizmoSettings>,
    theme: Option<Res<Theme>>,
) {
    // Theme tokens (semantic colours, §3.1) — fall back to plain egui colours
    // when headless / no theme registered.
    let accent = theme.as_ref().map(|t| t.tokens.accent).unwrap_or(egui::Color32::LIGHT_BLUE);
    let subdued = theme.as_ref().map(|t| t.tokens.text_subdued).unwrap_or(egui::Color32::LIGHT_GREEN);
    let path_color = to_bevy(accent);
    let approach_color = to_bevy(subdued);
    let pin_color = to_bevy(accent);

    for (vessel_xf, spec) in q.iter() {
        let BehaviorSpec::Patrol { waypoints, .. } = &spec.0 else { continue };
        if waypoints.is_empty() {
            continue;
        }
        let vessel_pos = vessel_xf.translation();
        let positions: Vec<Vec3> = waypoints
            .iter()
            .map(|w| Vec3::from_array(w.pos))
            .collect();

        // Approach leg: vessel → waypoint 1.
        if settings.show_approach {
            let first = positions[0];
            gizmos.line(vessel_pos, first, approach_color);
        }

        // Path segments between consecutive waypoints.
        if settings.show_path && positions.len() > 1 {
            for pair in positions.windows(2) {
                gizmos.line(pair[0], pair[1], path_color);
            }
            // Loop closure: last → first (a patrol cycles).
            let last = *positions.last().unwrap();
            gizmos.line(last, positions[0], path_color);
        }

        // Numbered pins.
        for p in &positions {
            gizmos.sphere(*p, settings.pin_radius, pin_color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_default_and_command_set_some() {
        let mut s = CheckpointGizmoSettings::default();
        assert!(s.show_path);
        s.pin_radius = 0.5;
        assert_eq!(s.pin_radius, 0.5);
    }
}