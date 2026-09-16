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
            let path = match entry.resolve(&twin.root) {
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
