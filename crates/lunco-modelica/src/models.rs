//! Bundled Modelica models for web deployment.
//!
//! Embeds .mo files at compile time so they're available in the browser
//! without requiring file system access.

/// Battery model - simple battery simulation
pub const BATTERY: &str = include_str!("../../../assets/models/Battery.mo");

/// BouncyBall model - ball bouncing with floor collision
pub const BOUNCY_BALL: &str = include_str!("../../../assets/models/BouncyBall.mo");

/// RC_Circuit model - resistor-capacitor circuit
pub const RC_CIRCUIT: &str = include_str!("../../../assets/models/RC_Circuit.mo");

/// SpringMass model - mass-spring-damper system
pub const SPRING_MASS: &str = include_str!("../../../assets/models/SpringMass.mo");

/// All bundled models as (name, source) pairs
pub const BUNDLED_MODELS: &[(&str, &str)] = &[
    ("Battery.mo", BATTERY),
    ("BouncyBall.mo", BOUNCY_BALL),
    ("RC_Circuit.mo", RC_CIRCUIT),
    ("SpringMass.mo", SPRING_MASS),
];

/// Get a bundled model by filename, returns None if not found
pub fn get_model(filename: &str) -> Option<&'static str> {
    BUNDLED_MODELS.iter()
        .find(|(name, _)| *name == filename)
        .map(|(_, source)| *source)
}
