//! Headless celestial mechanics and semantic reference-frame services.
//!
//! This package deliberately knows nothing about a scene representation. It
//! owns the analytical values that authored and runtime adapters can consume:
//! NAIF identity, ephemeris, IAU rotation, geodesy, Kepler propagation, and
//! frame conversion. The sibling `lunco-celestial-spatial` package owns the
//! scene/runtime projection.

pub mod components;
pub mod coords;
pub mod ephemeris;
pub mod frames;
pub mod geo;
pub mod iau;
pub mod kepler;
pub mod registry;
pub mod transform;

pub use components::*;
pub use ephemeris::*;
pub use geo::*;
pub use iau::*;
pub use kepler::*;
pub use registry::*;
pub use transform::*;
