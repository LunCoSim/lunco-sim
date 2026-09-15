//! Persisted source-library settings.
//!
//! Lives in `settings.json` under key `library`. Fields are optional so
//! the schema can evolve without invalidating existing settings files.
//!
//! - `local_root_override` — absolute path to a user-supplied source-library
//!   tree. Wins over the cached download.

use std::path::PathBuf;

use bevy::prelude::*;
use lunco_settings::SettingsSection;
use serde::{Deserialize, Serialize};

/// Persisted source-library settings (one slice of `settings.json`).
#[derive(Resource, Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
pub struct LibrarySettings {
    /// User-supplied path to a source-library tree, e.g. a system install or
    /// a local checkout. When set and pointing at a directory, the workbench
    /// uses it directly and skips the explicit download.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_root_override: Option<PathBuf>,
}

impl SettingsSection for LibrarySettings {
    const KEY: &'static str = "library";
}
