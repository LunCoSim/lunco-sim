//! Renderer selection and the per-image sampling contract. Neither owns physics.

use bevy::prelude::*;
use lunco_settings::SettingsSection;
use serde::{Deserialize, Serialize};

/// Stable renderer identifiers shared by settings and Twin overrides.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[serde(rename_all = "snake_case")]
pub enum RenderingBackend {
    /// The normal Bevy PBR and authored shader pipeline.
    #[default]
    Raster,
    /// Bevy's reference path tracer and native forward transparent effects,
    /// accumulated at a fixed scene state.
    PathTracer,
}

impl RenderingBackend {
    /// Stable identifier used by manifests and capture provenance.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Raster => "raster",
            Self::PathTracer => "path_tracer",
        }
    }
    /// Parse an authored identifier without silently selecting another renderer.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "raster" => Ok(Self::Raster),
            "path_tracer" => Ok(Self::PathTracer),
            _ => Err(format!(
                "unknown rendering backend `{value}`; expected raster or path_tracer"
            )),
        }
    }

    /// Stable human-readable settings label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Raster => "Raster (default)",
            Self::PathTracer => "Offline path tracing (experimental)",
        }
    }
}

/// Persisted application preference; the active Twin may override either field.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[reflect(Resource)]
#[serde(default)]
pub struct RendererSettings {
    /// Requested graphics backend. Raster is the default in ordinary sessions.
    pub backend: RenderingBackend,
    /// Independent path samples accumulated for each output image.
    pub samples_per_pixel: u32,
}

impl Default for RendererSettings {
    fn default() -> Self {
        Self {
            backend: RenderingBackend::Raster,
            samples_per_pixel: 128,
        }
    }
}

impl SettingsSection for RendererSettings {
    const KEY: &'static str = "renderer";

    fn validate_section(&self) -> Result<(), String> {
        if self.samples_per_pixel == 0 {
            Err("renderer.samples_per_pixel must be positive".into())
        } else {
            Ok(())
        }
    }
}

/// Projection of the active Twin's settings, cleared when its ownership ends.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct TwinRendererOverride {
    pub backend: Option<RenderingBackend>,
    pub samples_per_pixel: Option<u32>,
    /// Invalid authored values block selection and capture rather than falling back.
    pub error: Option<String>,
}

impl TwinRendererOverride {
    /// Resolve explicit Twin fields over the application's persisted preference.
    pub fn resolve(&self, settings: RendererSettings) -> Result<RendererSettings, String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let effective = RendererSettings {
            backend: self.backend.unwrap_or(settings.backend),
            samples_per_pixel: self.samples_per_pixel.unwrap_or(settings.samples_per_pixel),
        };
        effective.validate_section()?;
        Ok(effective)
    }
}

/// An image ticket held stable while the renderer gathers samples. The capture
/// owner issues a new revision only after its fixed-step and participant barriers.
#[derive(Resource, Default, Debug)]
pub struct OfflineImageSampling {
    revision: u64,
    pub requested: bool,
    pub completed_samples: u32,
    /// The renderer sets this only for a completed matching revision.
    pub ready: bool,
    pub error: Option<String>,
    /// A take keeps its renderer and sample target until it ends.
    pub capture_settings: Option<RendererSettings>,
}

impl lunco_core::SceneResource for OfflineImageSampling {
    fn clear_scene(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.requested = false;
        self.completed_samples = 0;
        self.ready = false;
        self.error = None;
        self.capture_settings = None;
    }
}

impl OfflineImageSampling {
    /// Begin a fresh accumulation; stale render acknowledgements cannot admit it.
    pub fn begin_image(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.requested = true;
        self.completed_samples = 0;
        self.ready = false;
        self.error = None;
    }

    /// Current ticket, passed through the render extraction boundary.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// End the request after capture or cancellation.
    pub fn clear(&mut self) {
        self.requested = false;
        self.ready = false;
    }

    /// Release the take-level configuration lock as well as its image ticket.
    pub fn end_capture(&mut self) {
        self.clear();
        self.capture_settings = None;
    }

    /// Accept only the current ticket's actual render progress.
    pub fn acknowledge(&mut self, revision: u64, samples: u32, required: u32) {
        if self.requested && revision == self.revision {
            self.completed_samples = samples;
            self.ready = samples >= required;
        }
    }
}

/// Observable renderer admission state, independent of simulation readiness.
#[derive(Resource, Default, Clone, Debug, PartialEq, Eq, Reflect)]
#[reflect(Resource)]
pub struct RendererStatus {
    pub requested: RenderingBackend,
    pub samples_per_pixel: u32,
    pub hardware_ray_tracing: bool,
    /// Transient material preparation. Capture waits; this is not a rejection.
    pub pending_reason: Option<String>,
    /// Unsupported or invalid rendering intent. Capture terminates visibly.
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_samples_cannot_finish_a_new_physics_image() {
        let mut image = OfflineImageSampling::default();
        image.begin_image();
        let old = image.revision();
        image.acknowledge(old, 128, 128);
        assert!(image.ready);
        image.begin_image();
        image.acknowledge(old, 256, 128);
        assert!(!image.ready);
        assert_eq!(image.completed_samples, 0);
        image.acknowledge(image.revision(), 127, 128);
        assert!(!image.ready);
        image.acknowledge(image.revision(), 128, 128);
        assert!(image.ready);
    }

    #[test]
    fn override_resolution_rejects_invalid_values() {
        let settings = RendererSettings::default();
        assert_eq!(
            TwinRendererOverride::default().resolve(settings).unwrap(),
            settings
        );
        let mut project = TwinRendererOverride {
            backend: Some(RenderingBackend::PathTracer),
            ..default()
        };
        assert_eq!(
            project.resolve(settings).unwrap().backend,
            RenderingBackend::PathTracer
        );
        project.samples_per_pixel = Some(0);
        assert!(project.resolve(settings).is_err());
        assert!(RenderingBackend::parse("automatic").is_err());
    }
}
