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

/// A camera-only arc between two orbit directions that preserves orbit radius.
#[derive(Component, Clone, Copy, Debug)]
pub struct OrbitCameraTransition {
    from: Quat,
    to: Quat,
    duration_s: f64,
    elapsed_s: f64,
    cancelled: bool,
}

impl OrbitCameraTransition {
    /// Create a transition between finite unit camera rotations.
    pub fn new(from: Quat, to: Quat, duration_s: f64) -> Option<Self> {
        if !from.is_finite()
            || !to.is_finite()
            || from.length_squared() <= f32::EPSILON
            || to.length_squared() <= f32::EPSILON
            || !duration_s.is_finite()
            || duration_s <= 0.0
        {
            return None;
        }
        Some(Self {
            from: from.normalize(),
            to: to.normalize(),
            duration_s,
            elapsed_s: 0.0,
            cancelled: false,
        })
    }

    /// Advance the eased arc and report whether it has reached its target.
    pub fn advance(&mut self, delta_s: f64) -> Option<(Quat, bool)> {
        if self.cancelled || !delta_s.is_finite() || delta_s < 0.0 {
            return None;
        }
        self.elapsed_s = (self.elapsed_s + delta_s).min(self.duration_s);
        let progress = (self.elapsed_s / self.duration_s).clamp(0.0, 1.0);
        let eased = (progress * progress * (3.0 - 2.0 * progress)) as f32;
        Some((self.from.slerp(self.to, eased).normalize(), progress >= 1.0))
    }

    /// Stop the arc when direct user look input takes control.
    pub fn cancel(&mut self) {
        self.cancelled = true;
    }
}

#[cfg(test)]
mod tests {
    use super::OrbitCameraTransition;
    use bevy::math::{Quat, Vec3};

    #[test]
    fn orbit_camera_transition_eases_through_the_arc_and_reaches_its_target() {
        let target = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let mut transition = OrbitCameraTransition::new(Quat::IDENTITY, target, 2.0)
            .expect("finite positive duration and rotations are valid");

        let (midpoint, finished) = transition.advance(1.0).expect("transition advances");
        let direction = midpoint.mul_vec3(Vec3::Z);
        assert!(!finished);
        assert!((direction.x - std::f32::consts::FRAC_1_SQRT_2).abs() < 1.0e-5);
        assert!((direction.z - std::f32::consts::FRAC_1_SQRT_2).abs() < 1.0e-5);

        let (final_rotation, finished) = transition.advance(1.0).expect("transition advances");
        assert!(finished);
        assert!(final_rotation.dot(target).abs() > 0.99999);
    }
}

/// Marks an orbital camera whose pose changed through local user input.
///
/// The avatar camera transaction owner consumes this marker when orbit mode ends
/// so a settled presentation pose can be retained without making the celestial
/// spatial writer depend on the avatar runtime.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct OrbitUserInput;

/// Shared exponential wheel sensitivity for avatar surface and orbital views.
pub const CAMERA_ZOOM_SENSITIVITY: f32 = 5.0;

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

    /// Remember a finite user-controlled pose for `body`.
    ///
    /// The boolean keeps invalid transient camera state visible to the owning
    /// adapter without making this low-level state container log or fabricate
    /// a replacement pose.
    pub fn remember_camera_pose(&mut self, body: i32, camera: &OrbitCamera) -> bool {
        let Some(pose) = OrbitPose::from_camera(camera) else {
            return false;
        };
        self.remember(body, pose);
        true
    }
}
