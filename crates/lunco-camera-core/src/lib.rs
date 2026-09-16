//! Backend-neutral camera rig contracts shared by avatar and scene systems.
//!
//! This crate contains camera state and pose contracts only. Specialized
//! `lunco-camera-runtime` provides generic pose realization, while specialized
//! runtimes such as `lunco-avatar` translate interaction and source-specific
//! frames into these contracts. Authored scene policy remains in USD/Rhai.

pub mod math;

/// Ordering anchor for generic interactive camera pose writers.
///
/// Avatar-specific preparation may run before this set and movement or other
/// pose consumers may run after it. The set belongs to the camera contract so
/// a host can compose camera realization without importing the avatar runtime.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub struct CameraUpdateSet;

/// Marker for an entity carrying a generic interactive camera rig.
///
/// Camera behavior components require this marker automatically. It lets
/// generic camera realization query the rig without identifying the product
/// that supplies it (avatar, inspection tool, or another authored operator).
#[derive(Component, Reflect, Debug, Default, Clone, Copy, PartialEq, Eq)]
#[reflect(Component)]
pub struct CameraRig;

/// Marker: this camera's pose is owned by an explicit pose driver.
///
/// Authored camera paths and direct camera commands use this fence to exclude
/// interactive pose writers. It is a camera contract, not a general engine
/// marker, so all camera runtimes share one owner and one reader path.
#[derive(Component, Reflect, Debug, Default, Clone, Copy, PartialEq, Eq)]
#[reflect(Component)]
pub struct CameraPoseLock;

use bevy::prelude::*;

/// Shared defaults for camera pose solvers.
///
/// Specialized runtimes may override these values on individual camera
/// components. Keeping the defaults with the camera contracts avoids making
/// an avatar runtime the owner of generic camera behavior.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct CameraDefaults {
    /// Base damping used when a camera component does not provide one.
    pub damping: f32,
    /// Base responsiveness (Hz) of rotation follow before damping scales it.
    pub rotation_rate: f32,
    /// Base responsiveness (Hz) of position follow before damping scales it.
    pub position_rate: f32,
    /// Default distance used by camera setup that does not author a distance.
    pub default_distance: f64,
}

impl Default for CameraDefaults {
    fn default() -> Self {
        Self {
            damping: 0.1,
            rotation_rate: 60.0,
            position_rate: 30.0,
            default_distance: 10.0,
        }
    }
}

/// Authored attitude policy for a target-following camera.
///
/// This is a camera contract, not a general engine component. USD projection
/// may attach it to a vessel, while a camera realization chooses the matching
/// pose calculation. An omitted value uses the stable heading behavior.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[reflect(Component)]
pub enum CameraFollow {
    /// Track the target's yaw heading while keeping its surface up direction.
    #[default]
    Heading,
    /// Track the target position in a stable external frame.
    Orbit,
    /// Follow the target's complete attitude.
    Chase,
}

/// Parse the authored `lunco:cameraFollow` token.
pub fn parse_camera_follow(s: &str) -> Option<CameraFollow> {
    match s.trim().to_ascii_lowercase().as_str() {
        "heading" => Some(CameraFollow::Heading),
        "orbit" => Some(CameraFollow::Orbit),
        "chase" => Some(CameraFollow::Chase),
        _ => None,
    }
}

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
    fn camera_follow_accepts_only_canonical_tokens() {
        assert_eq!(parse_camera_follow("heading"), Some(CameraFollow::Heading));
        assert_eq!(parse_camera_follow("orbit"), Some(CameraFollow::Orbit));
        assert_eq!(parse_camera_follow("CHASE"), Some(CameraFollow::Chase));
        for token in ["springarm", "yaw", "stable", "external", "cockpit", "full"] {
            assert_eq!(parse_camera_follow(token), None, "{token}");
        }
    }

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

/// Chase camera that follows a target with a spring-arm pose.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
#[require(CameraRig, CameraZoomInput)]
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
#[require(CameraRig, CameraZoomInput)]
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

/// Free-flight camera that moves independently of a target.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
#[require(CameraRig, FreeFlightSettings)]
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
#[require(CameraRig, FreeFlightSettings)]
pub struct SurfaceCamera {
    /// Heading from local north in radians.
    pub heading: f32,
    /// Elevation from the horizon in radians.
    pub pitch: f32,
}

/// Generic surface basis supplied by the subsystem that owns the surface.
///
/// Camera realization consumes this value without knowing whether it came
/// from celestial geodesy, a terrain frame, or another spatial provider. The
/// provider must replace it when the camera's parent frame or surface point
/// changes; missing or non-finite axes cause the pose writer to hold.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component)]
pub struct SurfaceCameraFrame {
    /// Tangent east axis in the camera's direct parent Grid.
    pub east: Vec3,
    /// Tangent north axis in the camera's direct parent Grid.
    pub north: Vec3,
    /// Surface-up axis in the camera's direct parent Grid.
    pub up: Vec3,
}

impl SurfaceCameraFrame {
    /// Create a frame only when all supplied axes are finite and non-zero.
    pub fn new(east: Vec3, north: Vec3, up: Vec3) -> Option<Self> {
        (east.is_finite()
            && north.is_finite()
            && up.is_finite()
            && east.length_squared() > f32::EPSILON
            && north.length_squared() > f32::EPSILON
            && up.length_squared() > f32::EPSILON)
            .then_some(Self { east, north, up })
    }
}

/// Marker for camera clipping-plane adaptation.
#[derive(Component, Reflect, Clone, Debug, Default)]
#[reflect(Component)]
pub struct AdaptiveNearPlane;

/// Marker for surface-relative camera and movement mode.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
pub struct SurfaceRelativeMode;
