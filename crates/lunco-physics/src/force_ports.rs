//! Avian force-port admission contracts.
//!
//! These names identify ports whose writes enter the Avian force accumulator.
//! The co-simulation port table consumes this contract when it classifies
//! destinations for realtime-safety admission; the force accumulator and its
//! application system remain in the Avian port adapter.

/// Avian body input ports that write world- or body-frame force accumulators.
pub const BODY_FORCE_PORTS: &[&str] = &[
    "force_x",
    "force_y",
    "force_z",
    "force_local_x",
    "force_local_y",
    "force_local_z",
    "torque_x",
    "torque_y",
    "torque_z",
];

/// Input ports that drive generic physical actuators.
pub const ACTUATOR_FORCE_PORTS: &[&str] = &["force_command", "torque_command"];

/// Return whether a port writes the Avian force accumulator.
pub fn is_physics_force_port(port: &str) -> bool {
    BODY_FORCE_PORTS.contains(&port) || ACTUATOR_FORCE_PORTS.contains(&port)
}

#[cfg(test)]
mod tests {
    use super::is_physics_force_port;

    #[test]
    fn force_ports_are_the_gated_ones() {
        assert!(is_physics_force_port("force_y"));
        assert!(is_physics_force_port("torque_z"));
        assert!(is_physics_force_port("force_local_x"));
        assert!(!is_physics_force_port("throttle"));
        assert!(is_physics_force_port("force_command"));
        assert!(is_physics_force_port("torque_command"));
        assert!(!is_physics_force_port("angle"));
        assert!(!is_physics_force_port("torque"));
    }
}
