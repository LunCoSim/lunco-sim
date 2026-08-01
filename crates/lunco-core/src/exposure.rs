//! Domain-free engine exposure registry.
//!
//! Engine producers publish named snapshots here. Consumers may be an HTML UI,
//! egui, an API stream, a remote client, or a diagnostic recorder. The registry
//! deliberately has no renderer dependency and changes only when an exposed
//! value changes.

use bevy::prelude::Resource;
use std::collections::HashMap;

/// Maximum presentation publication rate for reactive runtime consumers.
pub const EXPOSURE_UPDATE_HZ: f32 = 20.0;

/// A scalar or already-formatted value exposed by an engine capability.
#[derive(Debug, Clone, PartialEq)]
pub enum ExposureValue {
    Text(String),
    Bool(bool),
    Number(f64),
}

impl ExposureValue {
    pub fn render(&self) -> String {
        match self {
            Self::Text(value) => value.clone(),
            Self::Bool(value) => value.to_string(),
            Self::Number(value) => value.to_string(),
        }
    }
}

#[derive(Debug, Default)]
pub struct ExposureSurface {
    pub visible: bool,
    pub properties: HashMap<String, ExposureValue>,
}

/// Retained named exposure snapshots shared by engine and presentation
/// consumers. `revision` is the change-detection boundary; it is not a frame
/// counter and does not change for identical values.
#[derive(Resource, Debug)]
pub struct EngineExposures {
    pub surfaces: HashMap<String, ExposureSurface>,
    pub revision: u64,
}

impl Default for EngineExposures {
    fn default() -> Self {
        Self {
            surfaces: HashMap::new(),
            revision: 1,
        }
    }
}

/// Shared invalidation state for exposure producers. Individual producers own
/// their dependency/change queries; this resource coalesces them to a bounded
/// update cadence.
#[derive(Resource, Debug)]
pub struct ExposureRefresh {
    pub dirty: bool,
    pub first_update: bool,
}

impl Default for ExposureRefresh {
    fn default() -> Self {
        Self::new()
    }
}

impl ExposureRefresh {
    pub fn new() -> Self {
        Self {
            dirty: true,
            first_update: true,
        }
    }
}

/// Update writer scoped to one named exposure surface.
pub struct ExposureWriter<'a> {
    surface: &'a mut ExposureSurface,
    revision: &'a mut u64,
}

impl EngineExposures {
    pub fn writer(&mut self, namespace: &str) -> ExposureWriter<'_> {
        let surface = self.surfaces.entry(namespace.to_owned()).or_default();
        ExposureWriter {
            surface,
            revision: &mut self.revision,
        }
    }
}

impl ExposureWriter<'_> {
    pub fn visible(&mut self, value: bool) {
        if self.surface.visible != value {
            self.surface.visible = value;
            *self.revision = self.revision.wrapping_add(1);
        }
    }

    pub fn property(&mut self, name: &str, value: impl Into<ExposureValue>) {
        let value = value.into();
        if self.surface.properties.get(name) != Some(&value) {
            self.surface.properties.insert(name.to_owned(), value);
            *self.revision = self.revision.wrapping_add(1);
        }
    }
}

impl From<String> for ExposureValue {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for ExposureValue {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<bool> for ExposureValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<f64> for ExposureValue {
    fn from(value: f64) -> Self {
        Self::Number(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_changes_only_for_new_exposure_values() {
        let mut exposures = EngineExposures::default();
        let initial = exposures.revision;
        {
            let mut writer = exposures.writer("test");
            writer.visible(true);
            writer.property("speed", 2.0_f64);
            writer.property("speed", 2.0_f64);
        }
        let changed = exposures.revision;
        assert!(changed > initial);

        {
            let mut writer = exposures.writer("test");
            writer.visible(true);
            writer.property("speed", 2.0_f64);
        }
        assert_eq!(exposures.revision, changed);
    }
}
