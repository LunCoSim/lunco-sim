//! System, project, and Twin settings for finite-world physics containment.

use bevy::prelude::*;
use lunco_settings::SettingsSection;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Twin manifest key for finite-world escape containment.
pub const FINITE_WORLD_ESCAPE_GUARD_KEY: &str = "physics.finite_world_escape_guard";
pub(crate) const PHYSICS_SECTION: &str = "physics";
pub(crate) const FINITE_WORLD_ESCAPE_GUARD_FIELD: &str = "finite_world_escape_guard";

/// System-wide default for finite static-world escape containment.
///
/// This setting is deliberately off by default. A project or Twin may opt in.
#[derive(Resource, Reflect, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[reflect(Resource)]
#[serde(default)]
pub struct PhysicsEscapeGuardSettings {
    /// Pause dynamic objects that leave a finite static-world bound.
    pub finite_world_escape_guard: bool,
}

impl Default for PhysicsEscapeGuardSettings {
    fn default() -> Self {
        Self {
            finite_world_escape_guard: false,
        }
    }
}

impl SettingsSection for PhysicsEscapeGuardSettings {
    const KEY: &'static str = PHYSICS_SECTION;
}

/// Project-local override. `None` inherits the system setting.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectPhysicsEscapeSettings {
    /// Override finite static-world escape containment for this project.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finite_world_escape_guard: Option<bool>,
}

impl SettingsSection for ProjectPhysicsEscapeSettings {
    const KEY: &'static str = PHYSICS_SECTION;
}

/// The scope that supplied an effective finite-world escape setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysicsEscapeGuardSource {
    /// System-wide user setting.
    System,
    /// Project-local `.lunco/settings.json` override.
    Project,
    /// Shared Twin manifest override.
    Twin,
}

impl PhysicsEscapeGuardSource {
    /// Stable source label for presentation and diagnostics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Project => "project",
            Self::Twin => "twin",
        }
    }
}

/// Effective finite-world escape setting and its source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicsEscapeGuardDecision {
    /// Whether finite static-world bounds contain dynamic bodies.
    pub enabled: bool,
    /// Scope which supplied this decision.
    pub source: PhysicsEscapeGuardSource,
}

/// Resolve precedence while preserving `false` as an explicit override.
pub fn resolve_physics_escape_guard(
    system: bool,
    project: Option<bool>,
    twin: Option<bool>,
) -> PhysicsEscapeGuardDecision {
    let (enabled, source) = if let Some(value) = twin {
        (value, PhysicsEscapeGuardSource::Twin)
    } else if let Some(value) = project {
        (value, PhysicsEscapeGuardSource::Project)
    } else {
        (system, PhysicsEscapeGuardSource::System)
    };
    PhysicsEscapeGuardDecision { enabled, source }
}

/// Read the Twin's optional finite-world escape override without coercion.
pub fn twin_physics_escape_guard_setting(
    manifest: Option<&lunco_twin::TwinManifest>,
) -> Result<Option<bool>, String> {
    match manifest.and_then(|manifest| manifest.setting(FINITE_WORLD_ESCAPE_GUARD_KEY)) {
        None => Ok(None),
        Some(lunco_twin::TwinSettingValue::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(format!("{FINITE_WORLD_ESCAPE_GUARD_KEY} must be a boolean")),
    }
}

/// Read the project-local physics override. I/O belongs on the open worker.
pub fn read_project_physics_escape_settings(
    root: &Path,
) -> Result<ProjectPhysicsEscapeSettings, String> {
    lunco_settings::read_optional_section::<ProjectPhysicsEscapeSettings>(
        &crate::scene_settings::project_settings_path(root),
    )
    .map(|settings| settings.unwrap_or_default())
}
