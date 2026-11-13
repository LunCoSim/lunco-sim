//! Project-local settings for persisted workbench view state.

use bevy::prelude::*;
use lunco_settings::SettingsSection;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub(crate) const UI_SECTION: &str = "ui";
pub(crate) const RESTORE_VIEW_STATE_FIELD: &str = "restore_view_state";

/// Project override for saving and restoring the Twin's View layout.
/// `None` uses the default, which is enabled.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectUiSettings {
    /// Save and restore camera and graph windows, selections, and geometry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restore_view_state: Option<bool>,
}

impl ProjectUiSettings {
    /// Whether this Twin's View layout should be saved and restored.
    pub fn save_view_layout(&self) -> bool {
        self.restore_view_state.unwrap_or(true)
    }
}

impl SettingsSection for ProjectUiSettings {
    const KEY: &'static str = UI_SECTION;
}

/// Read the project's optional UI-state preference. I/O belongs on the open worker.
/// A malformed preference falls back to the documented default and does not
/// prevent the authored scene from opening.
pub fn read_project_ui_settings(root: &Path) -> ProjectUiSettings {
    match lunco_settings::read_optional_section::<ProjectUiSettings>(
        &crate::scene_settings::project_settings_path(root),
    ) {
        Ok(settings) => settings.unwrap_or_default(),
        Err(error) => {
            warn!("[workspace-view-state] ignoring invalid optional project UI settings: {error}");
            ProjectUiSettings::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_layout_saving_defaults_on_and_keeps_explicit_off() {
        assert!(ProjectUiSettings::default().save_view_layout());
        assert!(
            ProjectUiSettings {
                restore_view_state: Some(true),
            }
            .save_view_layout()
        );
        assert!(
            !ProjectUiSettings {
                restore_view_state: Some(false),
            }
            .save_view_layout()
        );
    }

    #[test]
    fn malformed_optional_ui_settings_do_not_block_project_open() {
        let root = tempfile::tempdir().unwrap();
        let settings_path = crate::scene_settings::project_settings_path(root.path());
        std::fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
        std::fs::write(settings_path, r#"{"ui":{"restore_view_state":"enabled"}}"#).unwrap();

        assert!(read_project_ui_settings(root.path()).save_view_layout());
    }
}
