//! Reusable celestial frame and surface-coordinate contracts.
//!
//! This package contains the small ECS-facing boundary shared by cameras,
//! avatars, networking, scripting, USD projection, and telemetry. It depends
//! on the existing semantic astronomy and generic BigSpace coordinate
//! packages, but not on the celestial runtime's terrain, rendering, physics,
//! link solver, networking, or asset integration.

mod components;
mod connectivity;
mod frame_index;
mod mission;
pub mod surface_frame;
mod surface_pose;
mod tracking;
mod trajectory;

pub use components::{
    celestial_declared, AuthoredBodyAlbedo, CelestialBodyDecl, CelestialSunPresentation,
    LocalGravityField, OrbitalViewPin, SolarSystemRoot,
};
pub use connectivity::{
    LinkGeometryPeer, LinkGeometryState, LinkNode, LinkOccluder, LinkPeer, LinkState, WifiNode,
    WifiPeer, WifiState,
};
pub use frame_index::{
    transform_pose_between_reference_frames, update_reference_frame_index, ReferenceFrameIndex,
};
pub use mission::{MissionDecl, MissionSpacecraftDecl, MissionTrajectoryDecl};
pub use surface_frame::{
    gravity_up_in_grid, surface_axes_for_grid_position, surface_axes_from_body_position,
    surface_axes_in_grid,
};
pub use surface_pose::{
    resolve_surface_pose, BodyFixedPosition, SitePosition, SurfacePose, SurfacePoseQuery,
};
pub use tracking::SolarTracked;
pub use trajectory::{TrajectoryFrame, TrajectoryPath, TrajectoryView};
