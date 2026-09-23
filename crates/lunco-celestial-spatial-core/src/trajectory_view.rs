//! Scene-authored parameters for derived trajectory views.

use bevy::prelude::Component;

use crate::TrajectoryFrame;

/// Authored parameters for one ephemeris trajectory view.
#[derive(Component, Debug, Clone)]
pub struct TrajectoryViewDecl {
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
    /// Coordinate-frame convention validated while projecting authored USD.
    pub frame: TrajectoryFrame,
    /// Whether the view is initially visible.
    pub user_visible: Option<bool>,
    /// Optional inclusive start epoch in Julian days.
    pub start_epoch_jd: Option<f64>,
    /// Optional inclusive end epoch in Julian days.
    pub end_epoch_jd: Option<f64>,
}
