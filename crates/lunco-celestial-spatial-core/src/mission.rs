//! Scene-authored mission projection contracts.
//!
//! These components carry only authored mission metadata. The celestial
//! runtime owns trajectory sampling, spacecraft placement, and presentation;
//! keeping the declarations here lets a USD projector publish the facts
//! without depending on that runtime.

use bevy::prelude::Component;

/// A scene-authored declaration that a mission should be shown.
#[derive(Component, Debug, Clone)]
pub struct MissionDecl {
    /// Stable mission id (`"artemis-2"`).
    pub id: String,
    /// Display name (`"Artemis II"`).
    pub name: String,
    /// One-line human description.
    pub description: String,
}

/// Authored parameters for one mission trajectory view.
#[derive(Component, Debug, Clone)]
pub struct MissionTrajectoryDecl {
    /// Display name of the trajectory.
    pub name: String,
    /// Ephemeris id of the body being tracked.
    pub tracked_id: i32,
    /// Ephemeris id of the reference body.
    pub reference_id: i32,
    /// RGBA presentation colour. USD authors this as `color4f`.
    pub color: [f32; 4],
    /// Sampling window in days.
    pub sampling_days: f64,
    /// Sampling step in days.
    pub sampling_step: f64,
    /// `"BodyFixed"` or `"Inertial"`.
    pub frame: String,
    /// Whether the view is initially visible.
    pub user_visible: Option<bool>,
    /// Optional inclusive start epoch in Julian days.
    pub start_epoch_jd: Option<f64>,
    /// Optional inclusive end epoch in Julian days.
    pub end_epoch_jd: Option<f64>,
}

/// Authored parameters for one mission spacecraft marker.
#[derive(Component, Debug, Clone)]
pub struct MissionSpacecraftDecl {
    /// Display name of the marker.
    pub name: String,
    /// Ephemeris id of the represented object.
    pub ephemeris_id: i32,
    /// Ephemeris id of the reference body.
    pub reference_id: i32,
    /// Presentation scale. USD authors this as `float`.
    pub scale: f32,
    /// Optional inclusive start epoch in Julian days.
    pub start_epoch_jd: Option<f64>,
    /// Optional inclusive end epoch in Julian days.
    pub end_epoch_jd: Option<f64>,
    /// Optional visibility marker radius in kilometres. USD authors this as `float`.
    pub marker_radius_km: Option<f32>,
    /// Optional picking radius in kilometres. USD authors this as `float`.
    pub hit_radius_km: Option<f32>,
    /// Optional RGBA presentation colour. USD authors this as `color4f`.
    pub marker_color: Option<[f32; 4]>,
}
