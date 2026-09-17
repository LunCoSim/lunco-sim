//! Reusable celestial frame and surface-coordinate contracts.
//!
//! This package contains the small ECS-facing boundary shared by cameras,
//! avatars, networking, scripting, USD projection, and telemetry. It depends
//! on the existing semantic astronomy and generic BigSpace coordinate
//! packages, but not on the celestial runtime's terrain, rendering, physics,
//! networking, or asset integration.

mod components;
mod frame_index;
pub mod surface_frame;
mod surface_pose;

pub use components::{
    AuthoredBodyAlbedo, CelestialBodyDecl, LocalGravityField, OrbitalViewPin, celestial_declared,
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
