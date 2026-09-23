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
pub mod surface_frame;
mod surface_pose;
mod tracking;
mod trajectory;
mod trajectory_view;

pub use components::{
    AuthoredBodyAlbedo, CelestialBodyDecl, CelestialSunPresentation, LocalGravityField,
    OrbitalViewPin, SolarSystemRoot, celestial_declared,
};
pub use connectivity::{
    LinkGeometryPeer, LinkGeometryState, LinkNode, LinkOccluder, LinkPeer, LinkState, WifiNode,
    WifiPeer, WifiState,
};
pub use frame_index::{
    ReferenceFrameIndex, transform_pose_between_reference_frames, update_reference_frame_index,
};
pub use surface_frame::{
    gravity_up_in_grid, surface_axes_for_grid_position, surface_axes_from_body_position,
    surface_axes_in_grid,
};
pub use surface_pose::{
    BodyFixedPosition, SitePosition, SurfacePose, SurfacePoseQuery, resolve_surface_pose,
};
pub use tracking::SolarTracked;
pub use trajectory::{TrajectoryFrame, TrajectoryPath, TrajectoryView};
pub use trajectory_view::TrajectoryViewDecl;
