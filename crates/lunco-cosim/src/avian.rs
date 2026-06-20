//! Avian physics as a co-simulation model.
//!
//! Avian is treated identically to any other simulation model:
//! it has inputs (forces) and outputs (state).

use bevy::prelude::*;
use avian3d::prelude::*;
use std::collections::HashMap;

/// Avian physics as a co-simulation model.
///
/// Auto-added to any entity with a [`avian3d::prelude::RigidBody`]. Exposes Avian state
/// as named outputs and accepts forces as named inputs.
///
/// ## Input Connectors
///
/// | Name       | Effect                                  |
/// |------------|-----------------------------------------|
/// | `force_x`  | `Forces::apply_force(DVec3::X * value)` |
/// | `force_y`  | `Forces::apply_force(DVec3::Y * value)` |
/// | `force_z`  | `Forces::apply_force(DVec3::Z * value)` |
/// | `torque_x` | `Forces::apply_torque(DVec3::X * value)`|
/// | `torque_y` | `Forces::apply_torque(DVec3::Y * value)`|
/// | `torque_z` | `Forces::apply_torque(DVec3::Z * value)`|
///
/// ## Output Connectors
///
/// | Name           | Source                            |
/// |----------------|-----------------------------------|
/// | `position_x`   | `Position.0.x`                   |
/// | `position_y`   | `Position.0.y`                    |
/// | `position_z`   | `Position.0.z`                    |
/// | `velocity_x`   | `LinearVelocity.0.x`             |
/// | `velocity_y`   | `LinearVelocity.0.y`             |
/// | `velocity_z`   | `LinearVelocity.0.z`             |
/// | `height`       | Alias for `position_y`           |
///
/// ## Manual Stepping
///
/// Avian can be stepped manually instead of relying on Bevy's fixed schedule:
///
/// ```rust,ignore
/// world.resource_mut::<Time<Physics>>().advance_by(dt);
/// world.try_schedule_scope(PhysicsSchedule, |world, schedule| {
///     schedule.run(world);
/// });
/// ```
///
/// This lets the co-simulation master control the exact step order.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct AvianSim {
    /// Input connectors — forces/torques from other models.
    ///
    /// Applied during [`systems::apply_forces::apply_sim_forces`]
    /// before the Avian physics step.
    pub inputs: HashMap<String, f64>,
    /// Output connectors — position, velocity, derived values.
    ///
    /// Read by [`systems::propagate::propagate_connections`] every frame.
    pub outputs: HashMap<String, f64>,
}

impl Default for AvianSim {
    fn default() -> Self {
        Self {
            inputs: HashMap::default(),
            outputs: HashMap::default(),
        }
    }
}

impl AvianSim {
    /// Input connector names for forces.
    pub const FORCE_INPUTS: &'static [&'static str] = &["force_x", "force_y", "force_z"];
    /// Input connector names for torques.
    pub const TORQUE_INPUTS: &'static [&'static str] = &["torque_x", "torque_y", "torque_z"];
    /// Output connector names for positions.
    pub const POSITION_OUTPUTS: &'static [&'static str] = &["position_x", "position_y", "position_z"];
    /// Output connector names for velocities.
    pub const VELOCITY_OUTPUTS: &'static [&'static str] = &["velocity_x", "velocity_y", "velocity_z"];
    /// Alias output connector names.
    pub const ALIAS_OUTPUTS: &'static [&'static str] = &["height"];

    /// Initialize output connectors with zeros.
    pub fn init_outputs(&mut self) {
        for name in Self::POSITION_OUTPUTS {
            self.outputs.insert(name.to_string(), 0.0);
        }
        for name in Self::VELOCITY_OUTPUTS {
            self.outputs.insert(name.to_string(), 0.0);
        }
        for name in Self::ALIAS_OUTPUTS {
            self.outputs.insert(name.to_string(), 0.0);
        }
    }

    /// Declare the force input connectors (value 0).
    ///
    /// Inputs are a model's public interface: declaring them up front lets the
    /// wiring fabric's strict resolver ([`crate::ports::write_port`]) find the
    /// slot and report a wire to any *un*declared input as dangling — instead of
    /// silently creating it. Only the connectors [`apply_sim_forces`] actually
    /// consumes are declared (torque is documented but not yet applied, so it is
    /// intentionally left undeclared — a wire to it is a real error worth
    /// surfacing).
    pub fn init_inputs(&mut self) {
        for name in Self::FORCE_INPUTS {
            self.inputs.insert(name.to_string(), 0.0);
        }
    }

    /// Read current Avian state into output connectors.
    ///
    /// Reads from [`Position`], [`LinearVelocity`] and derived values.
    pub fn read_state(
        &mut self,
        position: Option<&Position>,
        linear_velocity: Option<&LinearVelocity>,
    ) {
        if let Some(pos) = position {
            self.outputs.insert("position_x".into(), pos.0.x);
            self.outputs.insert("position_y".into(), pos.0.y);
            self.outputs.insert("position_z".into(), pos.0.z);
            self.outputs.insert("height".into(), pos.0.y); // alias
        }
        if let Some(lin_vel) = linear_velocity {
            self.outputs.insert("velocity_x".into(), lin_vel.0.x);
            self.outputs.insert("velocity_y".into(), lin_vel.0.y);
            self.outputs.insert("velocity_z".into(), lin_vel.0.z);
        }
    }

    /// Take accumulated force inputs, zeroing the slots.
    ///
    /// Returns [fx, fy, fz] and resets the values to 0 **without removing the
    /// keys** — the force inputs are declared ports ([`init_inputs`]) that must
    /// persist so next tick's [`propagate_connections`] can write them again
    /// through the strict resolver. (Removing them, as the old drain did, would
    /// make the strict write fail the following tick.)
    pub fn take_inputs(&mut self) -> [f64; 3] {
        let mut take = |key: &str| match self.inputs.get_mut(key) {
            Some(slot) => {
                let v = *slot;
                *slot = 0.0;
                v
            }
            None => 0.0,
        };
        [take("force_x"), take("force_y"), take("force_z")]
    }
}