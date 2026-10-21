//! Domain-free engine exposure registry.
//!
//! Engine producers publish named snapshots here. Consumers may be an HTML UI,
//! egui, an API stream, a remote client, or a diagnostic recorder. The registry
//! deliberately has no renderer dependency. Publication revisions track value
//! changes, namespace withdrawal and scene retirement.

use bevy::prelude::Resource;
use lunco_core::{GlobalEntityId, SceneResource};
use std::collections::{HashMap, HashSet};

/// Maximum presentation publication rate for reactive runtime consumers.
pub const EXPOSURE_UPDATE_HZ: f32 = 20.0;

/// A typed value exposed by an engine capability.
///
/// Collections stay typed across the engine/Rhai/UI boundary. Presentation
/// code may choose how to render them, but the exposure registry never flattens
/// authored arrays or records into numbered scalar slots.
#[derive(Debug, Clone, PartialEq)]
pub enum ExposureValue {
    Text(String),
    Bool(bool),
    Number(f64),
    Array(Vec<ExposureValue>),
    Map(Vec<(String, ExposureValue)>),
}

impl ExposureValue {
    /// Render one scalar value for a template property or CSS custom property.
    /// Structured values are intentionally not coerced into display text.
    pub fn render(&self) -> String {
        match self {
            Self::Text(value) => value.clone(),
            Self::Bool(value) => value.to_string(),
            Self::Number(value) => value.to_string(),
            Self::Array(_) | Self::Map(_) => String::new(),
        }
    }

    pub fn scalar_render(&self) -> Option<String> {
        match self {
            Self::Text(value) => Some(value.clone()),
            Self::Bool(value) => Some(value.to_string()),
            Self::Number(value) => Some(value.to_string()),
            Self::Array(_) | Self::Map(_) => None,
        }
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct ExposureSurface {
    /// Whether a validated projection is available for this source cohort.
    pub state: ExposureState,
    pub visible: bool,
    /// Canonical entity identity represented by this surface, when it is
    /// subject-scoped. Presentation policy uses this typed identity instead of
    /// matching names or serialised payloads.
    pub subject: Option<GlobalEntityId>,
    pub properties: HashMap<String, ExposureValue>,
}

/// Publication state of a retained capability view. Pending and failed views
/// carry no fabricated properties; ready views retain their last valid sample.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum ExposureState {
    /// The source cohort has no validated view yet.
    Pending,
    /// A complete validated view is retained.
    #[default]
    Ready,
    /// Preparation stopped with an actionable owner diagnostic.
    Failed(String),
}

impl ExposureSurface {
    /// Build a value in an immutable preparation transaction.
    pub fn set_property(&mut self, name: impl AsRef<str>, value: impl Into<ExposureValue>) -> bool {
        let name = name.as_ref();
        let value = value.into();
        if self.properties.get(name) == Some(&value) {
            return false;
        }
        self.properties.insert(name.to_owned(), value);
        true
    }
}

/// Retained named exposure snapshots shared by engine and presentation
/// consumers. `revision` is the change-detection boundary; it is not a frame
/// counter. Identical writes keep it stable; namespace withdrawal and scene
/// retirement advance it.
#[derive(Resource, Debug)]
pub struct EngineExposures {
    pub surfaces: HashMap<String, ExposureSurface>,
    pub revision: u64,
    application_surfaces: HashSet<String>,
}

impl Default for EngineExposures {
    fn default() -> Self {
        Self {
            surfaces: HashMap::new(),
            revision: 1,
            application_surfaces: HashSet::new(),
        }
    }
}

impl SceneResource for EngineExposures {
    fn clear_scene(&mut self) {
        let mut outgoing = std::mem::take(self);
        self.revision = outgoing.revision.wrapping_add(1);
        self.application_surfaces = std::mem::take(&mut outgoing.application_surfaces);
        self.surfaces = outgoing
            .surfaces
            .into_iter()
            .filter(|(namespace, _)| self.application_surfaces.contains(namespace))
            .collect();
    }
}

/// Shared invalidation state for exposure producers. Individual producers own
/// their dependency/change queries; this resource coalesces them to a bounded
/// update cadence and keeps unrelated surfaces out of a publication.
#[derive(Resource, Debug)]
pub struct ExposureRefresh {
    /// The driven-body surface depends on continuous pose, body, and
    /// telemetry inputs.
    pub driven_vessel_dirty: bool,
    /// Authored runtime surfaces contain live model values and their
    /// authored root topology.
    pub control_dirty: bool,
    /// The schema surface depends on selection and authored USD topology.
    pub schema_dirty: bool,
    /// The celestial capability surface depends on authored bodies and the
    /// orbital-view pin.
    pub celestial_dirty: bool,
    /// Progress/overlay surfaces depend only on their own producer resources.
    pub overlay_dirty: bool,
    pub first_update: bool,
}

impl Default for ExposureRefresh {
    fn default() -> Self {
        Self::new()
    }
}

impl SceneResource for ExposureRefresh {}

impl ExposureRefresh {
    pub fn new() -> Self {
        Self {
            driven_vessel_dirty: true,
            control_dirty: true,
            schema_dirty: true,
            celestial_dirty: true,
            overlay_dirty: true,
            first_update: true,
        }
    }

    /// Whether any publication domain has pending work.
    pub fn any_dirty(&self) -> bool {
        self.driven_vessel_dirty
            || self.control_dirty
            || self.schema_dirty
            || self.celestial_dirty
            || self.overlay_dirty
    }

    /// Clear all domain invalidation bits after the scheduler admits a
    /// publication. The producer may set them again on a later update.
    pub fn clear_dirty(&mut self) {
        self.driven_vessel_dirty = false;
        self.control_dirty = false;
        self.schema_dirty = false;
        self.celestial_dirty = false;
        self.overlay_dirty = false;
    }
}

/// Update writer scoped to one named exposure surface.
pub struct ExposureWriter<'a> {
    surface: &'a mut ExposureSurface,
    revision: &'a mut u64,
}

impl EngineExposures {
    /// Give an application-owned surface an explicit lifetime across scene
    /// replacement. Other snapshots expire at teardown, including hidden data.
    pub fn retain_across_scenes(&mut self, namespace: impl Into<String>) {
        self.application_surfaces.insert(namespace.into());
    }

    /// Publish a complete view in one revision, including removal of retired keys.
    pub fn replace_surface(&mut self, namespace: &str, surface: ExposureSurface) {
        if self.surfaces.get(namespace) != Some(&surface) {
            self.surfaces.insert(namespace.to_owned(), surface);
            self.revision = self.revision.wrapping_add(1);
        }
    }

    /// Retire a source identity before its name can be reused.
    pub fn remove_surface(&mut self, namespace: &str) {
        if self.surfaces.remove(namespace).is_some() {
            self.revision = self.revision.wrapping_add(1);
        }
    }

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

    pub fn property(&mut self, name: impl AsRef<str>, value: impl Into<ExposureValue>) {
        if self.surface.set_property(name, value) {
            *self.revision = self.revision.wrapping_add(1);
        }
    }

    /// Remove the previous projection before applying a replacement view model.
    ///
    /// A policy-produced surface may change its property set when the active
    /// Twin or subject changes. Clearing here prevents a retired property from
    /// remaining visible in a retained consumer.
    pub fn clear_properties(&mut self) {
        if !self.surface.properties.is_empty() {
            self.surface.properties.clear();
            *self.revision = self.revision.wrapping_add(1);
        }
    }

    /// Remove an optional property when its source identity is unavailable.
    pub fn remove_property(&mut self, name: impl AsRef<str>) {
        if self.surface.properties.remove(name.as_ref()).is_some() {
            *self.revision = self.revision.wrapping_add(1);
        }
    }

    /// Set the canonical entity represented by this surface.
    pub fn subject(&mut self, value: Option<GlobalEntityId>) {
        if self.surface.subject != value {
            self.surface.subject = value;
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
    fn scene_lifecycle_retires_surfaces_after_terminal_publications() {
        use bevy::prelude::*;
        use lunco_core::{AppSceneExt, SceneOwned, SceneTeardown, run_scene_teardown};
        #[derive(Component)]
        struct TerminalPublisher;
        let mut app = App::new();
        app.init_scene_resource::<EngineExposures>()
            .init_scene_resource::<ExposureRefresh>()
            .add_systems(SceneTeardown, |mut exposures: ResMut<EngineExposures>| {
                exposures
                    .writer("shutdown")
                    .property("source", "old source");
            })
            .add_observer(|_: On<Remove<TerminalPublisher>>, mut commands: Commands| {
                commands.queue(|world: &mut World| {
                    world
                        .resource_mut::<EngineExposures>()
                        .writer("removal")
                        .property("source", "old source");
                });
            });
        app.world_mut()
            .resource_mut::<EngineExposures>()
            .retain_across_scenes("application");
        app.world_mut()
            .resource_mut::<EngineExposures>()
            .writer("application")
            .property("status", "ready");
        for _ in 0..3 {
            app.world_mut().spawn((SceneOwned, TerminalPublisher));
            let revision = {
                let mut exposures = app.world_mut().resource_mut::<EngineExposures>();
                exposures.writer("scene").property("secret", "old source");
                exposures
                    .writer("hidden-scene")
                    .property("secret", "hidden old source");
                exposures.revision
            };
            {
                let mut refresh = app.world_mut().resource_mut::<ExposureRefresh>();
                refresh.clear_dirty();
                refresh.first_update = false;
            }
            run_scene_teardown(app.world_mut());
            let exposures = app.world().resource::<EngineExposures>();
            assert_eq!(exposures.surfaces.len(), 1);
            assert_eq!(
                exposures.surfaces["application"].properties["status"].render(),
                "ready"
            );
            assert!(exposures.revision > revision);
            let refresh = app.world().resource::<ExposureRefresh>();
            assert!(refresh.first_update && refresh.any_dirty());
        }
    }

    #[test]
    fn complete_surface_publication_is_atomic_and_retains_equal_revisions() {
        let mut registry = EngineExposures::default();
        registry.replace_surface(
            "subject",
            ExposureSurface {
                state: ExposureState::Pending,
                ..Default::default()
            },
        );
        let pending = registry.revision;
        let mut ready = ExposureSurface {
            visible: true,
            ..Default::default()
        };
        ready.set_property("value", 1.25);
        registry.replace_surface("subject", ready);
        assert!(registry.revision > pending);
        let revision = registry.revision;
        let mut same = ExposureSurface {
            visible: true,
            ..Default::default()
        };
        same.set_property("value", 1.25);
        registry.replace_surface("subject", same);
        assert_eq!(registry.revision, revision);
        registry.replace_surface(
            "subject",
            ExposureSurface {
                state: ExposureState::Failed("invalid policy".into()),
                ..Default::default()
            },
        );
        assert!(registry.surfaces["subject"].properties.is_empty());
        assert!(!registry.surfaces["subject"].visible);
    }

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

    #[test]
    fn refresh_clears_coalesced_domain_invalidations() {
        let mut refresh = ExposureRefresh::new();
        assert!(refresh.first_update);
        assert!(refresh.any_dirty());

        refresh.clear_dirty();
        refresh.first_update = false;
        assert!(!refresh.any_dirty());

        refresh.schema_dirty = true;
        refresh.overlay_dirty = true;
        assert!(refresh.any_dirty());
        refresh.clear_dirty();
        assert!(!refresh.any_dirty());
    }
}
