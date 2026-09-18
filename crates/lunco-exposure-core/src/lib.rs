//! Renderer-independent typed runtime exposure registry.

pub mod exposure;

pub use exposure::{
    EXPOSURE_UPDATE_HZ, EngineExposures, ExposureRefresh, ExposureSurface, ExposureValue,
    ExposureWriter,
};
