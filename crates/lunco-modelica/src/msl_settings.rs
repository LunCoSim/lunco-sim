//! Persisted MSL settings.
//!
//! Lives in `settings.json` under key `msl`. Fields are optional so
//! the schema can evolve without invalidating existing settings files.
//!
//! - `local_root_override` — absolute path to a user-supplied MSL
//!   tree (e.g. a system install, a checked-out Modelica repo). Wins
//!   over the cached download.
//! - `last_fetched_version` — bookkeeping populated from the Assets
//!   manifest entry after a successful user-requested download; surfaced
//!   in the Assets settings panel so the user can tell what's on disk.

use std::path::PathBuf;

use bevy::prelude::*;
use lunco_settings::SettingsSection;
use serde::{Deserialize, Serialize};

/// Persisted MSL settings (one slice of `settings.json`).
#[derive(Resource, Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
pub struct MslSettings {
    /// User-supplied path to an MSL tree, e.g. a system install or a
    /// local checkout. When set and pointing at a directory that
    /// contains `Modelica/`, the workbench uses it directly and skips
    /// the explicit download.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_root_override: Option<PathBuf>,

    /// Version string from `Assets.toml` `[msl].version` after the
    /// most recent successful user-requested download. Read-only display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fetched_version: Option<String>,
}

impl SettingsSection for MslSettings {
    const KEY: &'static str = "msl";
}
