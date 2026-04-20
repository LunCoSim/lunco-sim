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

/// Bundled example models, each with a one-line tagline that the
/// Welcome tab shows next to the "Learn by Example" button.
pub struct BundledModel {
    /// Filename (e.g. `"RocketEngine.mo"`).
    pub filename: &'static str,
    /// Embedded source.
    pub source: &'static str,
    /// Short description for the Welcome tab / tooltips.
    pub tagline: &'static str,
}

/// RocketEngine — simplified liquid rocket engine. Thrust from
/// propellant mass flow × exhaust velocity, integrates total impulse.
pub const ROCKET_ENGINE: &str = include_str!("../../../assets/models/RocketEngine.mo");

/// Battery — simple SOC integrator with configurable capacity + current.
pub const BATTERY: &str = include_str!("../../../assets/models/Battery.mo");

/// Single-stage RC circuit — source, resistor, capacitor, ground.
pub const RC_CIRCUIT: &str = include_str!("../../../assets/models/RC_Circuit.mo");

/// Two-stage RC low-pass filter — 6 MSL components + 7 connects.
/// The example to run when you want to see the Diagram view
/// actually render a block schematic.
pub const CASCADED_RC_FILTER: &str =
    include_str!("../../../assets/models/CascadedRCFilter.mo");

/// BouncyBall — projectile-under-gravity with ideal floor collisions.
pub const BOUNCY_BALL: &str = include_str!("../../../assets/models/BouncyBall.mo");

/// SpringMass — classic mass / spring / damper second-order system.
pub const SPRING_MASS: &str = include_str!("../../../assets/models/SpringMass.mo");

/// AnnotatedRocketStage — visual fixture for the graphical-annotation
/// pipeline (Icon/Diagram graphics + Placement). Engine, Tank, Gimbal
/// leaves with rich icons, placed on a composite RocketStage diagram.
pub const ANNOTATED_ROCKET_STAGE: &str =
    include_str!("../../../assets/models/AnnotatedRocketStage.mo");

/// All bundled example models, in Welcome-tab display order.
/// Order matters: the first entry is the one the web binary auto-opens.
pub const BUNDLED_MODELS: &[BundledModel] = &[
    BundledModel {
        filename: "CascadedRCFilter.mo",
        source: CASCADED_RC_FILTER,
        tagline: "Two-stage RC low-pass filter — 6 MSL blocks, renders as a schematic",
    },
    BundledModel {
        filename: "AnnotatedRocketStage.mo",
        source: ANNOTATED_ROCKET_STAGE,
        tagline: "Annotation visual fixture — Engine, Tank, Gimbal icons on a stage diagram",
    },
    BundledModel {
        filename: "RocketEngine.mo",
        source: ROCKET_ENGINE,
        tagline: "Liquid rocket — thrust, mass flow, total impulse (equation-only)",
    },
    BundledModel {
        filename: "Battery.mo",
        source: BATTERY,
        tagline: "Battery — state-of-charge integrator (equation-only)",
    },
    BundledModel {
        filename: "RC_Circuit.mo",
        source: RC_CIRCUIT,
        tagline: "Single-stage RC — the minimal schematic",
    },
    BundledModel {
        filename: "BouncyBall.mo",
        source: BOUNCY_BALL,
        tagline: "Projectile under gravity with floor collisions",
    },
    BundledModel {
        filename: "SpringMass.mo",
        source: SPRING_MASS,
        tagline: "Mass–spring–damper second-order system",
    },
];

/// Get a bundled model source by filename.
pub fn get_model(filename: &str) -> Option<&'static str> {
    BUNDLED_MODELS
        .iter()
        .find(|m| m.filename == filename)
        .map(|m| m.source)
}
