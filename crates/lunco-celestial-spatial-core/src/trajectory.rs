//! Render-independent trajectory facts projected from authored mission data.

use bevy::math::DVec3;
use bevy::prelude::*;

/// The reference-frame convention used when sampling a trajectory.
#[derive(Reflect, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TrajectoryFrame {
    /// Sample the relative inertial position of the tracked and reference bodies.
    #[default]
    Inertial,
    /// Rotate the sampled relative position into the reference body's fixed frame.
    BodyFixed,
}

/// Authored trajectory sampling and presentation intent.
#[derive(Component, Reflect, Clone, Copy, Debug)]
#[reflect(Component)]
pub struct TrajectoryView {
    /// Ephemeris id of the tracked object.
    pub tracked_id: i32,
    /// Ephemeris id of the reference object.
    pub reference_id: i32,
    /// Coordinate-frame convention for the sampled points.
    pub frame: TrajectoryFrame,
    /// Presentation colour selected by the authored declaration.
    pub color: LinearRgba,
    /// Whether the trajectory is eligible for visibility.
    pub is_visible: bool,
    /// Whether the user has enabled the trajectory overlay.
    pub user_visible: bool,
    /// Total sampling horizon in days.
    pub sampling_days: f64,
    /// Distance between consecutive samples in days.
    pub sampling_step: f64,
    /// Optional authored start epoch.
    pub start_epoch: Option<f64>,
    /// Optional authored end epoch.
    pub end_epoch: Option<f64>,
}

impl Default for TrajectoryView {
    fn default() -> Self {
        Self {
            tracked_id: lunco_celestial::ephemeris_id::EARTH,
            reference_id: lunco_celestial::ephemeris_id::SUN,
            frame: TrajectoryFrame::Inertial,
            color: LinearRgba::WHITE,
            is_visible: true,
            user_visible: true,
            sampling_days: 200.0,
            sampling_step: 1.0,
            start_epoch: None,
            end_epoch: None,
        }
    }
}

/// Sampled trajectory points and their spatial anchoring metadata.
#[derive(Component, Default, Reflect)]
#[reflect(Component)]
pub struct TrajectoryPath {
    /// Sampled points in the declared frame.
    pub points: Vec<DVec3>,
    /// Epoch at which the sample was committed.
    pub update_epoch: f64,
    /// Offset subtracted from every point for local mesh precision.
    pub anchor: DVec3,
    /// Whether the sampled points are relative to the tracked body's frame.
    pub anchored: bool,
    /// Monotonic revision of the committed sampled geometry.
    pub geometry_revision: u64,
}
