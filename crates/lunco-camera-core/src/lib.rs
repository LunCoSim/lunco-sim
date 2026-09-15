//! Backend-neutral camera rig contracts shared by avatar and scene systems.
//!
//! This crate contains camera state and pose contracts only. Specialized
//! runtimes such as `lunco-avatar` translate interaction into these contracts
//! and provide fast pose solvers; authored scene policy remains in USD/Rhai.

use bevy::prelude::*;
use big_space::prelude::CellCoord;
use lunco_environment::GravityBody;

/// Authored parameters for a free-flight camera rig.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component)]
pub struct FreeFlightSettings {
    /// Straight-line free-flight speed in stage metres per second.
    pub speed_mps: f64,
    /// Multiplier applied while the authored boost command is active.
    pub boost_multiplier: f64,
    /// Threshold in the normalized boost command that activates boost.
    pub boost_threshold: f64,
    /// Absolute movement-command deadzone.
    pub input_deadzone: f64,
}

impl Default for FreeFlightSettings {
    fn default() -> Self {
        Self {
            speed_mps: 23.1,
            boost_multiplier: 10.0,
            boost_threshold: 0.5,
            input_deadzone: 0.01,
        }
    }
}

/// Initial camera behavior authored for a camera rig in USD.
///
/// This is a data contract between the USD projection and the generic avatar
/// movement runtime. It deliberately contains no scenario policy: Rhai may
/// select, focus, or replace the active camera through the command surface.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CameraRigMode {
    /// Move independently in the active spatial frame.
    #[default]
    FreeFlight,
    /// Orbit a target in the target body's external frame.
    Orbit,
    /// Follow a target with a spring arm.
    SpringArm,
}

/// Current owner of a camera's spatial pose.
///
/// This is the cross-domain ownership contract for camera writers. USD
/// projection establishes the authored, interactive, mounted, or path-driven
/// role; a runtime camera command may establish an explicit pose. Pose writers
/// must only write while their corresponding mode is active.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[reflect(Component)]
pub enum CameraPoseMode {
    /// USD transform composition or animation owns the pose.
    #[default]
    Authored,
    /// An interactive camera rig owns the pose.
    Interactive,
    /// A mounted camera follower owns the pose.
    Mounted,
    /// An authored camera path owns the pose.
    Path,
    /// A direct runtime camera command owns the pose.
    Explicit,
}

/// Per-avatar mouse-wheel input accumulated between camera systems.
#[derive(Component, Default)]
pub struct CameraZoomInput {
    /// Accumulated scroll delta since the last camera system consumed it.
    pub delta: f32,
    transition_barrier: bool,
    transition_direction: Option<i8>,
    neutral_seconds: f32,
}

/// Idle interval that closes a camera-mode wheel handoff.
pub const CAMERA_ZOOM_HANDOFF_IDLE_SECS: f32 = 0.12;

impl CameraZoomInput {
    /// Start a semantic camera-mode handoff and consume the current gesture.
    pub fn begin_mode_transition(&mut self, direction: Option<f32>) {
        let already_barriered = self.transition_barrier;
        self.delta = 0.0;
        self.transition_barrier = true;
        if !already_barriered || direction.is_some() {
            self.transition_direction = direction
                .filter(|delta| delta.abs() > f32::EPSILON)
                .map(|delta| delta.signum() as i8);
        }
        self.neutral_seconds = 0.0;
    }

    /// Ingest one normalized wheel sample after the UI has accepted it.
    pub fn ingest(&mut self, delta: f32, accepted: bool, idle_seconds: f32) {
        if self.transition_barrier {
            if delta.abs() <= f32::EPSILON {
                self.neutral_seconds += idle_seconds.max(0.0);
                if self.neutral_seconds < CAMERA_ZOOM_HANDOFF_IDLE_SECS {
                    return;
                }
                self.transition_barrier = false;
                self.transition_direction = None;
            } else {
                self.neutral_seconds = 0.0;
                if !accepted {
                    return;
                }
                let direction = delta.signum() as i8;
                match self.transition_direction {
                    Some(previous) if previous == direction => return,
                    Some(_) => {}
                    None => return,
                }
                self.transition_barrier = false;
                self.transition_direction = None;
            }
        }
        if delta.abs() <= f32::EPSILON {
            return;
        }
        if accepted {
            self.delta += delta;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_transition_consumes_until_neutral() {
        let mut input = CameraZoomInput::default();

        input.ingest(-1.0, true, 1.0 / 60.0);
        assert_eq!(input.delta, -1.0);

        input.begin_mode_transition(Some(-1.0));
        input.ingest(-1.0, true, 1.0 / 60.0);
        assert_eq!(input.delta, 0.0);

        input.ingest(0.0, true, CAMERA_ZOOM_HANDOFF_IDLE_SECS);
        input.ingest(1.0, true, 1.0 / 60.0);
        assert_eq!(input.delta, 1.0);
    }

    #[test]
    fn transition_neutralizes_while_pointer_is_captured() {
        let mut input = CameraZoomInput::default();
        input.begin_mode_transition(None);

        input.ingest(1.0, false, 1.0 / 60.0);
        assert!(input.transition_barrier);

        input.ingest(0.0, false, CAMERA_ZOOM_HANDOFF_IDLE_SECS);
        input.ingest(1.0, false, 1.0 / 60.0);

        assert_eq!(input.delta, 0.0);
        assert!(!input.transition_barrier);
    }

    #[test]
    fn undirected_transition_consumes_until_neutral() {
        let mut input = CameraZoomInput::default();
        input.begin_mode_transition(None);

        input.ingest(1.0, true, 1.0 / 60.0);
        assert_eq!(input.delta, 0.0);
        assert!(input.transition_barrier);

        input.ingest(0.0, true, CAMERA_ZOOM_HANDOFF_IDLE_SECS);
        input.ingest(1.0, true, 1.0 / 60.0);

        assert_eq!(input.delta, 1.0);
        assert!(!input.transition_barrier);
    }

    #[test]
    fn return_transition_keeps_scroll_through_direction() {
        let mut input = CameraZoomInput::default();
        input.begin_mode_transition(Some(1.0));
        input.begin_mode_transition(None);

        input.ingest(1.0, true, CAMERA_ZOOM_HANDOFF_IDLE_SECS);

        assert_eq!(input.delta, 0.0);
        assert!(input.transition_barrier);
        assert_eq!(input.transition_direction, Some(1));
    }
}

/// Camera attitude mode used by the unified vessel-follow camera.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FollowAttitude {
    /// Follow the target's heading while keeping a stable up direction.
    #[default]
    Heading,
    /// Ignore target attitude and remain in the external frame.
    WorldLocked,
    /// Follow the complete target attitude, including roll.
    FullAttitude,
}

/// USD-authored initial camera and movement contract for an interactive rig.
///
/// USD owns the initial mode, pose, rig parameters, and flight parameters.
/// [`lunco-avatar`](https://docs.rs/lunco-avatar) only realizes this contract
/// into its generic ECS movement components; it does not choose a scene-specific
/// camera mode or hard-code a vehicle policy.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component)]
pub struct CameraRigIntent {
    /// Initial interactive camera mode.
    pub mode: CameraRigMode,
    /// Initial yaw in radians.
    pub yaw: f32,
    /// Initial pitch in radians.
    pub pitch: f32,
    /// Initial orbital arm length in metres.
    pub orbit_distance: f64,
    /// Initial spring-arm length in metres.
    pub spring_arm_distance: f64,
    /// Initial spring-arm vertical offset in metres.
    pub spring_arm_vertical_offset: f32,
    /// Whether the spring arm follows the target heading.
    pub spring_arm_track_heading: bool,
    /// How the spring arm derives its target attitude.
    pub spring_arm_attitude: FollowAttitude,
    /// Authored free-flight movement parameters.
    pub flight_settings: FreeFlightSettings,
    /// Authored photographic exposure, if present.
    pub exposure_ev100: Option<f32>,
}

impl Default for CameraRigIntent {
    fn default() -> Self {
        Self {
            mode: CameraRigMode::FreeFlight,
            yaw: std::f32::consts::PI * 0.8,
            pitch: -0.3,
            orbit_distance: 30.0,
            spring_arm_distance: 15.0,
            spring_arm_vertical_offset: 2.0,
            spring_arm_track_heading: true,
            spring_arm_attitude: FollowAttitude::Heading,
            flight_settings: FreeFlightSettings::default(),
            exposure_ev100: None,
        }
    }
}

/// Chase camera that follows a target with a spring-arm pose.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
#[require(CameraZoomInput)]
pub struct SpringArmCamera {
    /// Followed entity.
    pub target: Entity,
    /// Arm length in metres.
    pub distance: f64,
    /// User yaw offset in radians.
    pub yaw: f32,
    /// User pitch offset in radians.
    pub pitch: f32,
    /// Optional rotation damping.
    pub damping: Option<f32>,
    /// Vertical arm offset.
    pub vertical_offset: f32,
    /// Whether target heading drives the camera.
    pub track_heading: bool,
    /// How target attitude contributes to orientation.
    pub attitude: FollowAttitude,
}

/// Survey camera that orbits a target in an external frame.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
#[require(CameraZoomInput)]
pub struct OrbitCamera {
    /// Followed celestial or spacecraft entity.
    pub target: Entity,
    /// Orbit radius in metres.
    pub distance: f64,
    /// Orbit yaw in radians.
    pub yaw: f32,
    /// Orbit pitch in radians.
    pub pitch: f32,
    /// Optional rotation damping.
    pub damping: Option<f32>,
    /// Vertical orbit offset.
    pub vertical_offset: f32,
}

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

/// Free-flight camera that moves independently of a target.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
#[require(FreeFlightSettings)]
pub struct FreeFlightCamera {
    /// Camera yaw in radians.
    pub yaw: f32,
    /// Camera pitch in radians.
    pub pitch: f32,
    /// Optional movement damping.
    pub damping: Option<f32>,
}

/// Camera whose orientation is derived from a local surface frame.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
#[require(FreeFlightSettings)]
pub struct SurfaceCamera {
    /// Heading from local north in radians.
    pub heading: f32,
    /// Elevation from the horizon in radians.
    pub pitch: f32,
}

/// Marker for camera clipping-plane adaptation.
#[derive(Component, Reflect, Clone, Debug, Default)]
#[reflect(Component)]
pub struct AdaptiveNearPlane;

/// Marker for surface-relative camera and movement mode.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
pub struct SurfaceRelativeMode;
