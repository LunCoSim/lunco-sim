//! Bundled Modelica models for web deployment.
//!
//! ## Why this exists
//!
//! On desktop, models are loaded from `assets/models/*.mo` via `std::fs`.
//! On wasm32 the browser sandbox blocks filesystem access. These models are
//! embedded at compile time via `include_str!` so the web binary has zero
//! runtime file I/O.
//!
//! To add a new model:
//! 1. Place the `.mo` file in `assets/models/`
//! 2. Add a `pub const` here with `include_str!(...)`
//! 3. Add it to `BUNDLED_MODELS`
//!
//! On desktop this module is also available (same source, no conditional
//! compilation) so both binaries share the same model data.

/// Battery model — simple battery simulation.
pub const BATTERY: &str = include_str!("../../../assets/models/Battery.mo");

/// BouncyBall model — ball bouncing with floor collision.
pub const BOUNCY_BALL: &str = include_str!("../../../assets/models/BouncyBall.mo");

/// RC_Circuit model — resistor-capacitor circuit.
pub const RC_CIRCUIT: &str = include_str!("../../../assets/models/RC_Circuit.mo");

/// SpringMass model — mass-spring-damper system.
pub const SPRING_MASS: &str = include_str!("../../../assets/models/SpringMass.mo");

/// All bundled models as (filename, source) pairs.
/// Used by the web binary to pick a default model on startup.
pub const BUNDLED_MODELS: &[(&str, &str)] = &[
    ("Battery.mo", BATTERY),
    ("BouncyBall.mo", BOUNCY_BALL),
    ("RC_Circuit.mo", RC_CIRCUIT),
    ("SpringMass.mo", SPRING_MASS),
];

/// Get a bundled model by filename, returns `None` if not found.
pub fn get_model(filename: &str) -> Option<&'static str> {
    BUNDLED_MODELS.iter()
        .find(|(name, _)| *name == filename)
        .map(|(_, source)| *source)
}
