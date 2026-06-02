//! Centralised user settings.
//!
//! One file on disk (`~/.lunco/settings.json`), one resource in the
//! ECS, and a typed-section API that domain crates use to register
//! their own slice. The crate handles load-on-startup, persist-on-
//! change, and atomic disk writes — call sites just read & mutate
//! their `Res<MySection>` like any other resource.
//!
//! ## Why one file
//!
//! Per-feature files (`recents.json`, `perf_hud.json`, ...) make it
//! impossible to back up, sync, or hand-edit a user's preferences in
//! one place. VS Code / Blender / JetBrains all funnel everything
//! through one settings document; we follow the same shape.
//!
//! `recents.json` stays separate by design — it's high-churn list
//! state, not user prefs.
//!
//! ## Registering a section
//!
//! ```ignore
//! use lunco_settings::{AppSettingsExt, SettingsSection};
//! use serde::{Serialize, Deserialize};
//! use bevy::prelude::*;
//!
//! #[derive(Resource, Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
//! struct PerfHudSettings { enabled: bool }
//!
//! impl SettingsSection for PerfHudSettings {
//!     const KEY: &'static str = "perf_hud";
//! }
//!
//! fn build(app: &mut App) {
//!     app.add_plugins(lunco_settings::SettingsPlugin);
//!     app.register_settings_section::<PerfHudSettings>();
//! }
//! ```
//!
//! After that, mutate `ResMut<PerfHudSettings>` from any system; the
//! crate persists the change next frame.

use std::collections::BTreeMap;
use std::path::PathBuf;

use bevy::prelude::*;
use serde::{de::DeserializeOwned, Serialize};

/// A slice of `settings.json` owned by one feature.
///
/// Implementations live alongside the feature that owns them — e.g.
/// the perf HUD owns `PerfHudSettings`. Choose a stable [`KEY`] —
/// it's part of the on-disk schema and renaming it migrates badly.
///
/// [`KEY`]: SettingsSection::KEY
pub trait SettingsSection:
    Resource + Serialize + DeserializeOwned + Default + Clone + PartialEq + Send + Sync + 'static
{
    /// Stable JSON key under which this section is stored. Must be
    /// unique across all registered sections (collisions silently
    /// overwrite). Snake_case is conventional.
    const KEY: &'static str;
}

/// Resolved path to `settings.json`. Honours the `LUNCOSIM_CONFIG`
/// env override via [`lunco_assets::user_config_dir`].
pub fn settings_path() -> PathBuf {
    lunco_assets::user_config_dir().join("settings.json")
}

/// Read a single section directly from disk, **before** the App is
/// built. Used by plugins that need to gate *plugin registration*
/// itself on a persisted preference (e.g. only adding a heavy
/// diagnostic plugin when the user has the perf HUD turned on).
///
/// Returns `S::default()` when the file or key is missing — same
/// semantics as `register_settings_section`. Toggling the section at
/// runtime won't retro-actively register/unregister plugins; that
/// requires an app restart.
pub fn load_section_from_disk<S: SettingsSection>() -> S {
    let path = settings_path();
    use lunco_storage::Storage;
    let Ok(bytes) = lunco_storage::FileStorage::new()
        .read_sync(&lunco_storage::StorageHandle::File(path))
    else {
        return S::default();
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return S::default();
    };
    let raw: BTreeMap<String, serde_json::Value> = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return S::default(),
    };
    raw.get(S::KEY)
        .and_then(|v| serde_json::from_value::<S>(v.clone()).ok())
        .unwrap_or_default()
}

/// In-memory mirror of `settings.json`. Sections deserialize out of
/// `raw` on registration; the central flush serialises back into
/// `raw` and writes to disk when `dirty`.
#[derive(Resource, Default, Debug)]
pub struct Settings {
    raw: BTreeMap<String, serde_json::Value>,
    dirty: bool,
}

impl Settings {
    /// Read the raw JSON value for `key`, if any. Domain crates
    /// shouldn't need this — register a `SettingsSection` instead —
    /// but it's useful for the "advanced: edit raw JSON" UI escape
    /// hatch.
    pub fn raw(&self, key: &str) -> Option<&serde_json::Value> {
        self.raw.get(key)
    }

    /// Iterate registered keys and their current JSON values.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &serde_json::Value)> {
        self.raw.iter()
    }

    fn load_from_disk() -> Self {
        // On native, read `<config>/settings.json` from disk.
        // On wasm there is no filesystem; instead the same JSON blob
        // lives under `localStorage["lunco_settings"]`. Same shape on
        // both sides so every `SettingsSection` (Theme, panel layout,
        // perf HUD, …) round-trips identically across reloads.
        #[cfg(target_arch = "wasm32")]
        {
            let text = match wasm_storage_read() {
                Some(s) => s,
                None => return Self::default(),
            };
            let raw: BTreeMap<String, serde_json::Value> =
                serde_json::from_str(&text).unwrap_or_default();
            return Self { raw, dirty: false };
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            use lunco_storage::Storage;
            let path = settings_path();
            let text = match lunco_storage::FileStorage::new()
                .read_sync(&lunco_storage::StorageHandle::File(path))
            {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(s) => s,
                    Err(_) => return Self::default(),
                },
                Err(_) => return Self::default(),
            };
            let raw: BTreeMap<String, serde_json::Value> =
                serde_json::from_str(&text).unwrap_or_default();
            Self { raw, dirty: false }
        }
    }

    fn write_if_dirty(&mut self) {
        if !self.dirty {
            return;
        }
        // Serialise once; backends differ.
        let json = match serde_json::to_string_pretty(&self.raw) {
            Ok(s) => s,
            Err(e) => {
                warn!("[Settings] serialise failed: {e}");
                return;
            }
        };
        #[cfg(target_arch = "wasm32")]
        {
            // localStorage write is synchronous + small; quota is per
            // origin (~5 MB) and our settings JSON is on the order of
            // 100 bytes. Failure is logged once; the dirty bit clears
            // either way so we don't spam the console.
            if let Err(e) = wasm_storage_write(&json) {
                warn!("[Settings] localStorage write failed: {e}");
            }
            self.dirty = false;
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = settings_path();
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            // Write to a sibling tmp file and rename. `std::fs::write`
            // truncates the destination, which on Windows leaves a
            // zero-byte file if the process is killed mid-write (real
            // hazard during a power cut or hard close). `rename` over
            // an existing file is atomic on POSIX and on Windows ≥ 1607
            // — good enough for user settings.
            let tmp = path.with_extension("json.tmp");
            // Route the actual write through lunco-storage (clippy-banned
            // `std::fs::write`, wasm-incompatible); the tmp+rename atomicity
            // is preserved — `rename`/`create_dir_all` aren't on the ban list.
            use lunco_storage::Storage;
            if let Err(e) = lunco_storage::FileStorage::new()
                .write_sync(&lunco_storage::StorageHandle::File(tmp.clone()), json.as_bytes())
            {
                warn!("[Settings] write tmp {} failed: {e}", tmp.display());
                return;
            }
            if let Err(e) = std::fs::rename(&tmp, &path) {
                warn!(
                    "[Settings] atomic rename {} → {} failed: {e}",
                    tmp.display(),
                    path.display()
                );
                let _ = std::fs::remove_file(&tmp);
                return;
            }
            self.dirty = false;
        }
    }
}

/// Wasm-only: read the settings JSON blob from `localStorage`.
/// Returns `None` if there is no `window`, no `localStorage` (private
/// browsing), or the key is unset.
#[cfg(target_arch = "wasm32")]
fn wasm_storage_read() -> Option<String> {
    let win = web_sys::window()?;
    let storage = win.local_storage().ok().flatten()?;
    storage.get_item(WASM_STORAGE_KEY).ok().flatten()
}

/// Wasm-only: write the settings JSON blob into `localStorage`.
#[cfg(target_arch = "wasm32")]
fn wasm_storage_write(json: &str) -> Result<(), String> {
    let win = web_sys::window().ok_or("no window")?;
    let storage = win
        .local_storage()
        .map_err(|e| format!("local_storage(): {e:?}"))?
        .ok_or("local_storage is null (private browsing?)")?;
    storage
        .set_item(WASM_STORAGE_KEY, json)
        .map_err(|e| format!("setItem: {e:?}"))
}

/// Wasm-only: namespace key for the persisted settings JSON.
/// Mirrors the `KEY_PREFIX` convention in `wasm_autosave.rs` so all
/// our localStorage entries share an obvious prefix.
#[cfg(target_arch = "wasm32")]
const WASM_STORAGE_KEY: &str = "lunco_modelica/settings.json";

/// Adds the [`Settings`] resource (loaded from disk) and the central
/// flush system. Idempotent.
pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<Settings>() {
            app.insert_resource(Settings::load_from_disk());
            app.add_systems(Last, flush_settings);
        }
    }
}

/// Writes `Settings` to disk at the end of the frame when something
/// marked it dirty. Runs in `Last` so all section persisters have
/// already serialised into `raw` for this frame.
fn flush_settings(mut settings: ResMut<Settings>) {
    settings.write_if_dirty();
}

/// Extension trait for registering typed sections with the
/// [`Settings`] resource.
pub trait AppSettingsExt {
    /// Register a typed section.
    ///
    /// On registration, deserialises the section's slice out of the
    /// loaded `Settings` (or inserts the `Default` if absent), and
    /// adds a per-frame system that writes back to `Settings` when
    /// the resource changes.
    fn register_settings_section<S: SettingsSection>(&mut self) -> &mut Self;
}

impl AppSettingsExt for App {
    fn register_settings_section<S: SettingsSection>(&mut self) -> &mut Self {
        if !self.is_plugin_added::<SettingsPlugin>() {
            self.add_plugins(SettingsPlugin);
        }
        let initial: S = {
            let settings = self.world().resource::<Settings>();
            settings
                .raw
                .get(S::KEY)
                .and_then(|v| serde_json::from_value::<S>(v.clone()).ok())
                .unwrap_or_default()
        };
        self.insert_resource(initial);
        self.add_systems(Last, persist_section::<S>);
        self
    }
}

/// Per-section persister — when the typed resource changes,
/// re-serialise and stash the JSON value back into `Settings.raw`.
/// The central `flush_settings` system then writes the file.
fn persist_section<S: SettingsSection>(
    section: Res<S>,
    mut settings: ResMut<Settings>,
) {
    if !section.is_changed() {
        return;
    }
    let value = match serde_json::to_value(&*section) {
        Ok(v) => v,
        Err(e) => {
            warn!("[Settings:{}] serialise failed: {e}", S::KEY);
            return;
        }
    };
    if settings.raw.get(S::KEY) == Some(&value) {
        return;
    }
    settings.raw.insert(S::KEY.to_string(), value);
    settings.dirty = true;
}
