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
use serde::{de::DeserializeOwned, Deserialize, Serialize};

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
    // Same guard as `Settings::load_from_disk` — a test binary must not have its behaviour
    // decided by the developer's real config. This path is the sneakier of the two: it
    // reads the file *before the App exists*, to gate plugin registration.
    if !disk_backed() {
        return S::default();
    }
    // Same blob the App-built `Settings` resource loads, read before the App
    // exists — through the Storage API on both targets (native
    // `<config>/settings.json`; wasm the `localStorage` mirror via
    // `WebStorage`). Missing key / unreadable / bad UTF-8 / bad JSON all fall
    // back to `S::default()`. Mirrors `Settings::load_from_disk`.
    let Ok(bytes) = lunco_storage::read_file_sync(&settings_path()) else {
        return S::default();
    };
    let text = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return S::default(),
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
        // A test binary MUST NOT read the developer's real settings — otherwise a value
        // some other test persisted (or that the developer set by hand) decides how this
        // test behaves. See `disk_backed`.
        if !disk_backed() {
            return Self::default();
        }
        // One path for native and wasm: read the settings blob through the
        // Storage API. Native resolves `<config>/settings.json` on the local
        // filesystem; wasm maps the same path onto a `localStorage` key via
        // `WebStorage`. Same shape on both sides so every `SettingsSection`
        // (Theme, panel layout, perf HUD, …) round-trips identically.
        let path = settings_path();
        let text = match lunco_storage::read_file_sync(&path) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(s) => s,
                Err(_) => return Self::default(),
            },
            Err(_) => return Self::default(),
        };
        match serde_json::from_str(&text) {
            Ok(raw) => Self { raw, dirty: false },
            Err(e) => {
                // A corrupt/hand-mistyped settings blob used to be parsed to an
                // empty map and then silently overwritten on the next flush —
                // vaporising the user's prefs. Preserve it as `settings.json.bad`
                // (through the Storage API, so it works on both targets) before
                // falling back to defaults.
                let bad = path.with_extension("json.bad");
                warn!(
                    "[Settings] {} is not valid JSON ({e}); preserving as {} and starting fresh",
                    path.display(), bad.display()
                );
                if let Err(e) = lunco_storage::write_file_sync(&bad, text.as_bytes()) {
                    warn!("[Settings] could not preserve corrupt settings to {}: {e}", bad.display());
                }
                Self::default()
            }
        }
    }

    fn write_if_dirty(&mut self) {
        if !self.dirty {
            return;
        }
        // THE GUARD. A test binary must never write the developer's real settings file.
        // Clear the dirty bit so we don't re-attempt (and the in-memory value still
        // reflects the change — only persistence is suppressed).
        if !disk_backed() {
            self.dirty = false;
            return;
        }
        let json = match serde_json::to_string_pretty(&self.raw) {
            Ok(s) => s,
            Err(e) => {
                warn!("[Settings] serialise failed: {e}");
                // Serialising the same `raw` will fail identically next frame —
                // clear the dirty bit so we don't retry (and re-warn) forever.
                self.dirty = false;
                return;
            }
        };
        // One path for native and wasm: persist through the Storage API
        // (CQ-107/CQ-701). Native gets an atomic tmp+rename (no zero-byte file
        // on a mid-write crash) + parent-dir creation; wasm writes the
        // `localStorage` mirror via `WebStorage`. No raw `std::fs` / `web_sys`.
        let path = settings_path();
        if let Err(e) = lunco_storage::write_file_sync(&path, json.as_bytes()) {
            warn!("[Settings] write {} failed: {e}", path.display());
            return;
        }
        self.dirty = false;
    }
}

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
            let mut settings = self.world_mut().resource_mut::<Settings>();
            match settings.raw.get(S::KEY).cloned() {
                None => S::default(),
                Some(v) => match serde_json::from_value::<S>(v.clone()) {
                    Ok(s) => s,
                    Err(e) => {
                        let bad_key = format!("{}.bad", S::KEY);
                        warn!(
                            "[Settings:{}] stored section failed to parse ({e}); preserving it as \"{bad_key}\" and using defaults",
                            S::KEY
                        );
                        settings.raw.insert(bad_key, v);
                        settings.dirty = true;
                        S::default()
                    }
                },
            }
        };
        self.insert_resource(initial);
        self.add_systems(Last, persist_section::<S>);
        self
    }
}

/// Is this process a `cargo test` binary?
///
/// Cargo builds test/bench binaries into `target/<profile>/deps/`; real application
/// binaries live one level up (`target/<profile>/<name>`), examples in `examples/`, and an
/// installed binary anywhere else. Nothing legitimately *runs* an app from `deps/`, so
/// "my parent directory is named `deps`" identifies a test harness without libtest
/// cooperating (it sets no env var we could read).
#[cfg(not(target_arch = "wasm32"))]
fn is_test_binary() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.parent()
                .map(|dir| dir.file_name() == Some(std::ffi::OsStr::new("deps")))
        })
        .unwrap_or(false)
}

#[cfg(target_arch = "wasm32")]
fn is_test_binary() -> bool {
    // No `current_exe` in a browser, and no cargo-test harness either.
    false
}

/// Whether the settings plane may touch the filesystem at all.
///
/// **This is a safety gate, and it defaults to SAFE in tests.**
///
/// `register_settings_section` auto-adds [`SettingsPlugin`], which loads `settings.json`
/// from the user's real config dir and installs a flush system that writes it back on any
/// change. That is correct for the app and *actively dangerous* in a test: a test app that
/// merely installs a domain plugin inherits real, persistent, cross-process state. A
/// `lunco-telemetry` test flipped `TelemetrySettings::enabled` to `false`; that `false`
/// landed in the developer's real `~/.lunco/settings.json`, and every subsequent test in
/// the process — and the developer's next real run of the app — read it back. It presented
/// as a cluster of unrelated failures whose membership *changed with the test-thread
/// count*, because the poison travelled through the filesystem rather than the code.
///
/// So: a test binary is **in-memory only** — no read, no write — unless it explicitly names
/// a config dir via `LUNCOSIM_CONFIG` (which is how a test that genuinely wants to exercise
/// persistence opts in, pointing at a temp dir; see [`isolate_config_dir_for_tests`]).
///
/// Nine crates register settings sections. Gating here means none of them has to remember.
fn disk_backed() -> bool {
    // An explicit config dir is an explicit choice — honour it. Tests that want to test
    // persistence set it to a throwaway path.
    if std::env::var_os("LUNCOSIM_CONFIG").is_some() {
        return true;
    }
    !is_test_binary()
}

/// Point the settings plane at a throwaway config directory.
///
/// Mostly unnecessary now — [`disk_backed`] already makes a test binary in-memory by
/// default. Use this only when a test needs settings to genuinely *round-trip through a
/// file* (persistence tests), pointing at a temp dir rather than the real config.
///
/// Settings persist automatically: [`persist_section`] fires on *any* change to the typed
/// resource, and `flush_settings` then writes `settings.json`. In a test that means a
/// plugin under test which mutates its own settings resource **writes into the developer's
/// real `~/.lunco/settings.json`** — and the next test app, and their next real run of the
/// application, load it back.
///
/// This is not hypothetical. A `lunco-telemetry` test flipped `TelemetrySettings::enabled`
/// to `false`, that `false` landed in the real user config, and every subsequent test in
/// the process read it back and sampled nothing. It presented as a cluster of unrelated
/// failures whose membership *changed with the test-thread count*, because the poison
/// travelled through the filesystem rather than through the code.
///
/// Idempotent and safe to call from every test.
pub fn isolate_config_dir_for_tests(tag: &str) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join(format!("lunco-test-config-{tag}"));
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::remove_file(dir.join("settings.json"));
        // `lunco_assets::user_config_dir()` reads this first — see its docs.
        std::env::set_var("LUNCOSIM_CONFIG", &dir);
    });
}

/// Per-section persister — when the typed resource changes,
/// re-serialise and stash the JSON value back into `Settings.raw`.
/// The central `flush_settings` system then writes the file.
///
/// NOTE: this fires in tests too, writing to whatever `user_config_dir()` resolves to —
/// the developer's real config unless a test called [`isolate_config_dir_for_tests`].
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

/// Persisted user profile settings (e.g. username).
#[derive(Resource, serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
pub struct ProfileSettings {
    pub username: String,
}

impl Default for ProfileSettings {
    fn default() -> Self {
        Self {
            username: "Player".to_string(),
        }
    }
}

impl SettingsSection for ProfileSettings {
    const KEY: &'static str = "profile";
}

/// Persisted terrain/ground settings.
#[derive(Resource, serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
pub struct TerrainSettings {
    /// If false, custom terrain shaders (such as procedural regolith FBM)
    /// are disabled and fall back to the simple flat-lit/unlit geomorph shader.
    pub enable_shaders: bool,
}

impl Default for TerrainSettings {
    fn default() -> Self {
        Self {
            enable_shaders: true,
        }
    }
}

impl SettingsSection for TerrainSettings {
    const KEY: &'static str = "terrain";
}

/// Persistent settings for asset downloading.
#[derive(Resource, Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct DownloadSettings {
    /// Maximum number of concurrent asset downloads (default: 3).
    pub max_parallel_downloads: usize,
}

impl Default for DownloadSettings {
    fn default() -> Self {
        Self {
            max_parallel_downloads: 3,
        }
    }
}

impl SettingsSection for DownloadSettings {
    const KEY: &'static str = "download";
}


#[cfg(test)]
mod disk_guard_tests {
    use super::*;

    /// Self-verifying: this assertion runs INSIDE a cargo-test binary, so if the detector
    /// is right it must say so. If cargo ever stops building test binaries into `deps/`,
    /// this test fails loudly rather than the guard silently opening up and letting the
    /// whole suite write to the developer's real config again.
    #[test]
    fn a_test_binary_is_detected_as_such() {
        assert!(
            is_test_binary(),
            "the settings disk-guard no longer recognises a cargo-test binary — every \
             SettingsSection in the workspace is now free to overwrite the developer's \
             real ~/.lunco/settings.json"
        );
    }

    /// THE GUARD. With no explicit `LUNCOSIM_CONFIG`, a test process must be in-memory
    /// only. This is what stops one test's `enabled: false` from persisting into the
    /// developer's config and poisoning every later test in the process.
    #[test]
    fn a_test_process_does_not_touch_the_real_config_by_default() {
        // Only meaningful when the env override is absent — which is the state a plain
        // `cargo test` runs in.
        if std::env::var_os("LUNCOSIM_CONFIG").is_none() {
            assert!(!disk_backed(), "a test binary must never read or write the real settings");
        }
    }

    /// A dirty in-memory Settings must NOT write when the guard is closed — and must clear
    /// its dirty bit so it doesn't retry every frame.
    #[test]
    fn write_if_dirty_is_a_noop_under_the_guard() {
        if std::env::var_os("LUNCOSIM_CONFIG").is_some() {
            return; // persistence is explicitly enabled; nothing to assert here
        }
        let mut s = Settings::default();
        s.raw.insert("telemetry".into(), serde_json::json!({ "enabled": false }));
        s.dirty = true;
        s.write_if_dirty();
        assert!(!s.dirty, "the dirty bit must clear so we don't retry the suppressed write");
    }

    /// An explicitly-named config dir re-enables persistence — that is how a test that
    /// genuinely wants a file round-trip opts in, pointing at a temp path.
    #[test]
    fn an_explicit_config_dir_re_enables_persistence() {
        // Deliberately not mutating the process env here (it is global and would race with
        // the tests above). Assert the policy directly.
        assert!(
            disk_backed() == std::env::var_os("LUNCOSIM_CONFIG").is_some() || !is_test_binary(),
            "LUNCOSIM_CONFIG must be the opt-in for a test binary"
        );
    }
}
