//! Scene-owned camera image intent; physical equipment remains in Modelica.
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CameraImageMode {
    #[default]
    Color,
    Depth,
}

/// Parameters stay f64 until the explicit renderer/GPU conversion.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Component, Default)]
#[serde(default)]
pub struct CameraImageEffects {
    pub mode: CameraImageMode,
    pub near_m: f64,
    pub far_m: f64,
    pub read_noise: f64,
    pub shot_noise: f64,
    pub row_noise: f64,
    pub dropout: f64,
    /// Zero disables quantization; otherwise 2..65536 intensity levels.
    pub quantization_levels: u32,
    pub seed: u32,
    pub animated: bool,
    pub shader: String,
}
impl Default for CameraImageEffects {
    fn default() -> Self {
        Self {
            mode: CameraImageMode::Color,
            near_m: 0.1,
            far_m: 100.0,
            read_noise: 0.0,
            shot_noise: 0.0,
            row_noise: 0.0,
            dropout: 0.0,
            quantization_levels: 0,
            seed: 1,
            animated: true,
            shader: "lunco://shaders/camera_sensor.wesl".into(),
        }
    }
}
impl CameraImageEffects {
    pub fn validate(&self) -> Result<(), String> {
        if !self.near_m.is_finite()
            || !self.far_m.is_finite()
            || self.near_m < 0.0
            || self.far_m - self.near_m < 0.001
            || self.far_m > 1.0e7
        {
            return Err("Camera depth range requires 0 <= near < far <= 10000000 metres, separated by at least 0.001 m".into());
        }
        if [
            self.read_noise,
            self.shot_noise,
            self.row_noise,
            self.dropout,
        ]
        .iter()
        .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return Err("Camera noise and dropout must be finite fractions in [0, 1]".into());
        }
        if self.quantization_levels != 0 && !(2..=65536).contains(&self.quantization_levels) {
            return Err("Camera quantization is 0 (disabled) or 2..65536 levels".into());
        }
        // GPU-boundary admission: preserve authored f64 values, but reject a
        // display interval that would collapse when uploaded as f32.
        let near_gpu = self.near_m as f32;
        let far_gpu = self.far_m as f32;
        if near_gpu >= far_gpu {
            return Err("Camera depth interval collapses at GPU precision".into());
        }
        if self.shader.trim().is_empty()
            || !(self.shader.ends_with(".wesl") || self.shader.ends_with(".wgsl"))
        {
            return Err("Camera image effects require an explicit WESL/WGSL shader asset".into());
        }
        Ok(())
    }
    pub fn enabled(&self) -> bool {
        self.mode == CameraImageMode::Depth
            || self.read_noise > 0.0
            || self.shot_noise > 0.0
            || self.row_noise > 0.0
            || self.dropout > 0.0
            || self.quantization_levels > 0
            || self.shader != "lunco://shaders/camera_sensor.wesl"
    }
}

/// Render acknowledgement for one exact image configuration and target size.
/// Replacing the token prevents an outgoing render frame from admitting a
/// capture after configuration or allocation changes. This is presentation state.
#[derive(Component, Clone)]
pub struct CameraImagePresentation {
    pub settings: CameraImageEffects,
    pub dimensions: Option<UVec2>,
    ready: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl CameraImagePresentation {
    pub fn new(settings: CameraImageEffects, dimensions: Option<UVec2>) -> Self {
        Self {
            settings,
            dimensions,
            ready: Default::default(),
        }
    }
    pub fn is_ready(&self) -> bool {
        self.ready.load(std::sync::atomic::Ordering::Acquire)
    }
    pub fn mark_presented(&self) {
        self.ready.store(true, std::sync::atomic::Ordering::Release);
    }
}
