//! Twin-scoped native hook-provider lifecycle.
//!
//! This module is an application composition adapter, not part of the hook
//! substrate. The manifest is the Twin's explicit approval boundary; the
//! loader admits only providers whose capabilities match existing reflected
//! installable hooks. A failed optional provider is retained in the report and
//! does not terminate Twin loading or leave a partial registration behind.

use bevy::prelude::*;
use lunco_hooks_native::{NativePlugin, NativePluginHost};
use lunco_workspace::Twin;
use lunco_workspace::TwinId;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Resolve the provider through the asset owner's canonical containment check
/// before the native loader can execute code from it.
fn contained_provider_path(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let relative = path.strip_prefix(root).map_err(|_| {
        format!(
            "native provider {} is outside Twin {}",
            path.display(),
            root.display()
        )
    })?;
    lunco_assets_core::existing_path_within_root(root, relative)
        .map_err(|error| format!("cannot resolve native provider {}: {error}", path.display()))?
        .ok_or_else(|| format!("native provider {} does not exist", path.display()))
}

/// The active native-provider set and its latest load diagnostics.
#[derive(Resource)]
pub struct NativeTwinPlugins {
    host: NativePluginHost,
    /// Providers currently owned by the active Twin.
    pub loaded: Vec<NativePlugin>,
    /// Provider ids that failed admission or loading during the last sync.
    pub failed: Vec<String>,
    /// Twin owning `loaded`.
    pub active_twin: Option<TwinId>,
}

impl Default for NativeTwinPlugins {
    fn default() -> Self {
        Self {
            host: NativePluginHost::default(),
            loaded: Vec::new(),
            failed: Vec::new(),
            active_twin: None,
        }
    }
}

impl NativeTwinPlugins {
    /// Remove every provider owned by the current Twin.
    pub fn unload(&mut self) {
        self.loaded.clear();
        self.failed.clear();
        self.active_twin = None;
    }

    /// Load the explicitly enabled native providers from one Twin manifest.
    ///
    /// The report is returned to the lifecycle system, while its loaded
    /// providers and failure ids remain in this resource so diagnostics and
    /// UI/API consumers can distinguish an empty provider set from a failed
    /// declaration without inspecting logs.
    pub fn load_for_twin(&mut self, twin_id: TwinId, twin: &Twin) -> NativePluginLoadReport {
        self.unload();
        self.active_twin = Some(twin_id);
        let Some(manifest) = twin.manifest.as_ref() else {
            return NativePluginLoadReport::default();
        };

        let mut report = NativePluginLoadReport::default();
        let mut ids = HashSet::new();
        for entry in &manifest.native_plugins {
            if !entry.enabled {
                continue;
            }
            if !ids.insert(entry.id.clone()) {
                self.failed.push(entry.id.clone());
                report.failed.push(format!(
                    "{}: duplicate native plugin id in Twin manifest",
                    entry.id
                ));
                continue;
            }
            let path = match entry
                .resolve(&twin.root)
                .and_then(|path| contained_provider_path(&twin.root, &path))
            {
                Ok(path) => path,
                Err(error) => {
                    self.failed.push(entry.id.clone());
                    report.failed.push(format!("{}: {error}", entry.id));
                    continue;
                }
            };
            match self.host.load(&path) {
                Ok(plugin) if plugin.id() == entry.id => {
                    report.loaded.push(plugin.id().to_string());
                    self.loaded.push(plugin);
                }
                Ok(plugin) => {
                    let actual = plugin.id().to_string();
                    drop(plugin);
                    self.failed.push(entry.id.clone());
                    report
                        .failed
                        .push(format!("{}: descriptor identity is `{actual}`", entry.id));
                }
                Err(error) => {
                    self.failed.push(entry.id.clone());
                    report.failed.push(format!("{}: {error}", entry.id));
                }
            }
        }
        report
    }
}

/// Result of one Twin native-provider synchronization.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NativePluginLoadReport {
    /// Provider ids loaded and registered under their existing hook ids.
    pub loaded: Vec<String>,
    /// Human-readable failures; each entry remains visible to the owner.
    pub failed: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_paths_are_canonical_and_confined_to_the_twin() {
        let twin = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let provider = twin.path().join("провајдер with spaces.dll");
        let external = outside.path().join("external.dll");
        lunco_storage::write_file_sync(&provider, b"provider fixture").unwrap();
        lunco_storage::write_file_sync(&external, b"outside fixture").unwrap();
        assert_eq!(
            contained_provider_path(twin.path(), &provider).unwrap(),
            lunco_storage::canonicalize_file_path(&provider).unwrap()
        );
        assert!(contained_provider_path(twin.path(), &external).is_err());
        assert!(contained_provider_path(twin.path(), &twin.path().join("missing.dll")).is_err());

        #[cfg(unix)]
        {
            let internal_link = twin.path().join("internal.dll");
            let external_link = twin.path().join("external.dll");
            lunco_storage::create_file_symlink_sync(&provider, &internal_link).unwrap();
            lunco_storage::create_file_symlink_sync(&external, &external_link).unwrap();
            assert_eq!(
                contained_provider_path(twin.path(), &internal_link).unwrap(),
                lunco_storage::canonicalize_file_path(&provider).unwrap()
            );
            assert!(contained_provider_path(twin.path(), &external_link).is_err());
            let external_directory = twin.path().join("external-directory");
            lunco_storage::create_directory_symlink_sync(outside.path(), &external_directory)
                .unwrap();
            assert!(
                contained_provider_path(twin.path(), &external_directory.join("external.dll"))
                    .is_err()
            );
        }
    }
}
