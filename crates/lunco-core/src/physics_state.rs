//! Physics admission lifecycle markers.
//!
//! These markers describe whether a dynamic physics participant has published
//! its authored initial state. They are engine lifecycle facts, not port
//! endpoint contracts, so they remain in `lunco-core` while the generic port
//! surface lives in `lunco-port-core`.

use bevy::prelude::*;

/// Marks a dynamic physics participant while its authored initial state is
/// still being admitted into the live solver.
///
/// This is the inverse lifecycle state of [`PhysicsStateReady`]. Consumers
/// must not sample or record a dynamic body until the USD projection has
/// published its authored pose and velocity and the body has been promoted
/// from its admission state.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component)]
pub struct PhysicsStatePending;

/// Marks the boundary at which a physics participant has published its
/// authored initial state to the co-simulation fabric.
///
/// This is distinct from a port surface being ready: a rigid body can expose
/// its velocity and attitude ports while it is still being held kinematic
/// during articulated-scene admission. Sensors use this fact to acquire their
/// first live sample without treating the loader's zero-valued placeholder as
/// a physical measurement.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component)]
pub struct PhysicsStateReady;
