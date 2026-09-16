//! Avatar-specific camera transition contracts.
//!
//! Generic camera modes and pose math live in [`lunco_camera_core`]. This
//! package owns only the state needed to leave an avatar's interactive orbit
//! and restore its previous BigSpace branch and behavior.

use bevy::prelude::*;
use big_space::prelude::CellCoord;
use lunco_camera_core::{FreeFlightCamera, SpringArmCamera, SurfaceCamera};
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
