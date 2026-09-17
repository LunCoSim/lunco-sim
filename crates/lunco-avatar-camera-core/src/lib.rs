//! Avatar-specific camera transition contracts.
//!
//! Generic camera modes and pose math live in [`lunco_camera_core`]. This
//! package owns the state needed to leave an avatar's interactive orbit, retain
//! its transient presentation history, and restore its previous BigSpace branch
//! and behavior.

use bevy::prelude::*;
use big_space::prelude::CellCoord;
use lunco_camera_core::{FreeFlightCamera, OrbitCamera, SpringArmCamera, SurfaceCamera};
use lunco_environment::GravityBody;

/// Camera behavior captured before entering an orbital view.
#[derive(Clone, Debug)]
pub enum OrbitReturnBehavior {
    /// Restore a chase camera.
    SpringArm(SpringArmCamera),
    /// Restore a surface camera.
    Surface(SurfaceCamera),
    /// Restore free flight.
    FreeFlight(FreeFlightCamera),
}

/// Exact pre-orbit camera state used by the avatar transition system.
#[derive(Component, Clone, Debug)]
pub struct OrbitViewReturn {
    parent_grid: Entity,
    cell: CellCoord,
    transform: Transform,
    behavior: OrbitReturnBehavior,
    gravity_body: Option<GravityBody>,
    surface_relative: bool,
}

impl OrbitViewReturn {
    /// Capture a camera state before moving it to an inertial orbit grid.
    pub fn new(
        parent_grid: Entity,
        cell: CellCoord,
        transform: Transform,
        behavior: OrbitReturnBehavior,
        gravity_body: Option<GravityBody>,
        surface_relative: bool,
    ) -> Self {
        Self {
            parent_grid,
            cell,
            transform,
            behavior,
            gravity_body,
            surface_relative,
        }
    }

    /// Grid parent captured at orbit entry.
    pub fn parent_grid(&self) -> Entity {
        self.parent_grid
    }

    /// Cell-local coordinate captured at orbit entry.
    pub fn cell(&self) -> CellCoord {
        self.cell
    }

    /// Local transform captured at orbit entry.
    pub fn transform(&self) -> Transform {
        self.transform
    }

    /// Camera behavior captured at orbit entry.
    pub fn behavior(&self) -> &OrbitReturnBehavior {
        &self.behavior
    }

    /// Gravity binding captured at orbit entry.
    pub fn gravity_body(&self) -> Option<GravityBody> {
        self.gravity_body
    }

    /// Whether surface-relative mode was active at orbit entry.
    pub fn surface_relative(&self) -> bool {
        self.surface_relative
    }
}

/// Marks an orbit camera whose first pose faces the body's current region.
#[derive(Component, Debug, Clone, Copy)]
pub struct CurrentRegionArrival;

/// Marks an orbit camera whose arm is derived from its current position.
#[derive(Component, Debug, Clone, Copy)]
pub struct RadialArrival;

/// Marks an orbital camera whose pose changed through local user input.
///
/// The avatar transition owner consumes this marker when orbit mode ends so a
/// settled presentation pose can be retained without making the celestial
/// spatial writer depend on the avatar runtime.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct OrbitUserInput;

/// Shared exponential wheel sensitivity for avatar surface and orbital views.
pub const CAMERA_ZOOM_SENSITIVITY: f32 = 5.0;

/// Surface altitude at which the avatar's continuous wheel gesture enters or
/// leaves the celestial orbital view.
pub const SURFACE_ORBIT_HANDOFF_ALTITUDE_M: f64 = 50_000.0;

/// One settled user-controlled pose for a celestial body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitPose {
    yaw: f32,
    pitch: f32,
    distance: f64,
    damping: Option<f32>,
    vertical_offset: f32,
}

impl OrbitPose {
    /// Capture a finite, usable pose from an orbital camera.
    pub fn from_camera(camera: &OrbitCamera) -> Option<Self> {
        let pose = Self {
            yaw: camera.yaw,
            pitch: camera.pitch,
            distance: camera.distance,
            damping: camera.damping,
            vertical_offset: camera.vertical_offset,
        };
        (pose.yaw.is_finite()
            && pose.pitch.is_finite()
            && pose.distance.is_finite()
            && pose.distance > 0.0
            && pose.vertical_offset.is_finite()
            && pose.damping.is_none_or(f32::is_finite))
        .then_some(pose)
    }

    /// Stored yaw in radians.
    pub fn yaw(&self) -> f32 {
        self.yaw
    }

    /// Stored pitch in radians.
    pub fn pitch(&self) -> f32 {
        self.pitch
    }

    /// Stored radial distance in metres.
    pub fn distance(&self) -> f64 {
        self.distance
    }

    /// Stored optional camera damping.
    pub fn damping(&self) -> Option<f32> {
        self.damping
    }

    /// Stored vertical offset in metres.
    pub fn vertical_offset(&self) -> f32 {
        self.vertical_offset
    }
}

/// Per-avatar orbital presentation history, keyed by stable body identity.
///
/// This state is local to the avatar: orbital poses are user presentation
/// state, not a scene-wide celestial fact.
#[derive(Component, Clone, Debug, Default)]
pub struct OrbitViewHistory {
    poses: Vec<(i32, OrbitPose)>,
}

impl OrbitViewHistory {
    /// Return the last settled pose remembered for `body`.
    pub fn pose(&self, body: i32) -> Option<OrbitPose> {
        self.poses
            .iter()
            .find_map(|(id, pose)| (*id == body).then_some(*pose))
    }

    /// Remember a settled pose for `body`.
    pub fn remember(&mut self, body: i32, pose: OrbitPose) {
        if let Some((_, stored)) = self.poses.iter_mut().find(|(id, _)| *id == body) {
            *stored = pose;
        } else {
            self.poses.push((body, pose));
        }
    }
}
