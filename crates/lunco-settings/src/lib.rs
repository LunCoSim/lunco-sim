//! Centralised user settings.
//!
//! One file on disk in the OS config directory (`lunco/settings.json`), one resource in the
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
/// it is the authoritative on-disk section name.
///
/// [`KEY`]: SettingsSection::KEY
pub trait SettingsSection:
    Resource + Serialize + DeserializeOwned + Default + Clone + PartialEq + Send + Sync + 'static
{
    /// Stable JSON key under which this section is stored. Must be
    /// unique across all registered sections (collisions silently
    /// overwrite). Snake_case is conventional.
    const KEY: &'static str;

    /// Validate semantic invariants after JSON deserialization.
    ///
    /// Serde verifies the shape and scalar types of a section, but it cannot
    /// know domain rules such as positive ranges or mutually consistent limits.
    /// Returning an error rejects the stored section and makes registration use
    /// the section's documented default instead of exposing invalid values to
    /// runtime systems. Sections without semantic constraints keep the default.
    fn validate_section(&self) -> Result<(), String> {
        Ok(())
    }
}

/// Resolves the user-level configuration directory for LunCoSim.
///
/// This is the single owner of the configuration path used by settings,
/// recents, identities, layouts, and other per-user state. It is separate
/// from the regenerable asset cache owned by `lunco-assets`.
pub fn user_config_dir() -> PathBuf {
    if let Some(val) = std::env::var_os("LUNCOSIM_CONFIG") {
        return PathBuf::from(val);
    }
    if let Some(cfg) = dirs::config_dir() {
        return cfg.join("lunco");
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".lunco");
    }
    PathBuf::from(".lunco")
}

/// Returns a named user-configuration subdirectory, creating it for callers
/// that are about to write. Read-only callers should use [`user_config_dir`]
/// directly and append their probe path.
pub fn user_config_subdir(name: &str) -> PathBuf {
    let dir = if name.is_empty() {
        user_config_dir()
    } else {
        user_config_dir().join(name)
    };
    #[cfg(not(target_arch = "wasm32"))]
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Resolved path to `settings.json`.
pub fn settings_path() -> PathBuf {
    user_config_dir().join("settings.json")
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
        .filter(|section| section.validate_section().is_ok())
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
                    path.display(),
                    bad.display()
                );
                if let Err(e) = lunco_storage::write_file_sync(&bad, text.as_bytes()) {
                    warn!(
                        "[Settings] could not preserve corrupt settings to {}: {e}",
                        bad.display()
                    );
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
    /// loaded `Settings` (or uses the current `Default` if absent or
    /// invalid), removes an invalid stored value, and adds a system
    /// that writes the current section back when the resource changes.
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
                Some(v) => match serde_json::from_value::<S>(v) {
                    Ok(s) if s.validate_section().is_ok() => s,
                    Err(_) => {
                        settings.raw.remove(S::KEY);
                        settings.dirty = true;
                        S::default()
                    }
                    Ok(_) => {
                        settings.raw.remove(S::KEY);
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

/// Environment variable that forces the settings plane in-memory for this process.
///
/// See [`use_ephemeral_settings`] — set through that function, never by hand.
pub const EPHEMERAL_SETTINGS_VAR: &str = "LUNCOSIM_EPHEMERAL_SETTINGS";

/// Declare that this process must never write the user's settings file.
///
/// [`is_test_binary`] catches `cargo test` harnesses because they live in
/// `target/<profile>/deps/`. It does **not** catch a harness shipped as a
/// `[[bin]]` — `scene_test` sits at `target/<profile>/scene_test`, shaped
/// exactly like the real app, and so inherited full read-write access to the
/// developer's real `settings.json`.
///
/// That is not hypothetical. `scene_test` inserts
/// `CelestialCadenceSettings::EXACT` (tolerance 0°) for determinism;
/// [`persist_section`] wrote that 0 to disk, and every subsequent run of the
/// *sandbox* loaded it back. The celestial cadence gate — whose entire purpose
/// is to skip a ~10 ms/frame ephemeris solve — then compared `delta >= 0.0`,
/// which is a tautology, and solved every frame forever. It read as a broken
/// run condition; it was a poisoned settings file, and it travelled through the
/// filesystem across processes and across days.
///
/// Call this **before** building the `App` (before any
/// [`AppSettingsExt::register_settings_section`], which is what auto-adds
/// [`SettingsPlugin`] and performs the initial load). It gates the whole plane,
/// so a harness declares it once regardless of how many sections it registers.
pub fn use_ephemeral_settings() {
    // SAFETY-adjacent: must be called before the App is built, i.e. before any
    // settings system spawns a thread that could read the environment.
    unsafe { std::env::set_var(EPHEMERAL_SETTINGS_VAR, "1") };
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
/// landed in the developer's real settings file, and every subsequent test in
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
    // Checked FIRST, ahead of `LUNCOSIM_CONFIG`: this is the safety direction. A
    // harness that declared itself ephemeral must stay ephemeral even if the
    // ambient environment also names a config dir.
    if std::env::var_os(EPHEMERAL_SETTINGS_VAR).is_some() {
        return false;
    }
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
/// real settings file** — and the next test app, and their next real run of the
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
        // `user_config_dir()` reads this first — see its docs.
        std::env::set_var("LUNCOSIM_CONFIG", &dir);
    });
}

/// Per-section persister — when the typed resource changes,
/// re-serialise and stash the JSON value back into `Settings.raw`.
/// The central `flush_settings` system then writes the file.
///
/// NOTE: this fires in tests too, writing to whatever `user_config_dir()` resolves to —
/// the developer's real config unless a test called [`isolate_config_dir_for_tests`].
fn persist_section<S: SettingsSection>(section: Res<S>, mut settings: ResMut<Settings>) {
    if !section.is_changed() {
        return;
    }
    if let Err(reason) = section.validate_section() {
        warn!(
            "[Settings:{}] refusing to persist invalid runtime section: {reason}",
            S::KEY
        );
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
    /// Radius around a visual-detail camera that the terrain streamer refines
    /// aggressively. A camera marker can override this for a specific view.
    #[serde(default = "TerrainSettings::default_visual_detail_radius_m")]
    pub visual_detail_radius_m: f64,
    /// Additional distance for retaining already-refined camera terrain. This
    /// is hysteresis only: it avoids fine-to-coarse-to-fine churn while the
    /// camera moves, without requesting new detail outside the radius above.
    #[serde(default = "TerrainSettings::default_visual_detail_hysteresis_m")]
    pub visual_detail_hysteresis_m: f64,
}

impl TerrainSettings {
    const DEFAULT_VISUAL_DETAIL_RADIUS_M: f64 = 60.0;
    const DEFAULT_VISUAL_DETAIL_HYSTERESIS_M: f64 = 45.0;

    fn default_visual_detail_radius_m() -> f64 {
        Self::DEFAULT_VISUAL_DETAIL_RADIUS_M
    }

    fn default_visual_detail_hysteresis_m() -> f64 {
        Self::DEFAULT_VISUAL_DETAIL_HYSTERESIS_M
    }
}

impl Default for TerrainSettings {
    fn default() -> Self {
        Self {
            visual_detail_radius_m: Self::DEFAULT_VISUAL_DETAIL_RADIUS_M,
            visual_detail_hysteresis_m: Self::DEFAULT_VISUAL_DETAIL_HYSTERESIS_M,
        }
    }
}

impl SettingsSection for TerrainSettings {
    const KEY: &'static str = "terrain";
}

/// Persistent settings for asset downloading.
#[derive(Resource, Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct DownloadSettings {
    /// Maximum number of concurrent asset downloads (default: 3).
    pub max_parallel_downloads: usize,
    /// Maximum total attempts for one network operation, including the first
    /// request (default: 5).
    pub max_attempts: usize,
    /// Delay after the first failed attempt, in seconds (default: 1).
    pub retry_initial_delay_secs: u64,
    /// Exponential retry multiplier (default: 2).
    pub retry_backoff_multiplier: u32,
    /// Maximum delay between attempts, in seconds (default: 60).
    pub retry_max_delay_secs: u64,
}

impl DownloadSettings {
    pub const MAX_PARALLEL_DOWNLOADS_RANGE: std::ops::RangeInclusive<usize> = 1..=64;
    pub const MAX_ATTEMPTS_RANGE: std::ops::RangeInclusive<usize> = 1..=20;
    pub const RETRY_INITIAL_DELAY_SECS_RANGE: std::ops::RangeInclusive<u64> = 0..=3600;
    pub const RETRY_BACKOFF_MULTIPLIER_RANGE: std::ops::RangeInclusive<u32> = 2..=10;
    pub const RETRY_MAX_DELAY_SECS_RANGE: std::ops::RangeInclusive<u64> = 1..=86400;

    /// Delay before the next attempt after `failed_attempt`, where the first
    /// failed attempt is numbered one. The cap prevents a large retry budget
    /// from creating an unbounded wait.
    pub fn retry_delay(&self, failed_attempt: usize) -> std::time::Duration {
        let exponent = failed_attempt.saturating_sub(1).min(63);
        let multiplier = u128::from(self.retry_backoff_multiplier);
        let base = u128::from(self.retry_initial_delay_secs);
        let cap = u128::from(self.retry_max_delay_secs);
        let seconds = base
            .saturating_mul(multiplier.saturating_pow(exponent as u32))
            .min(cap);
        std::time::Duration::from_secs(seconds as u64)
    }
}

impl Default for DownloadSettings {
    fn default() -> Self {
        Self {
            max_parallel_downloads: 3,
            max_attempts: 5,
            retry_initial_delay_secs: 1,
            retry_backoff_multiplier: 2,
            retry_max_delay_secs: 60,
        }
    }
}

impl SettingsSection for DownloadSettings {
    const KEY: &'static str = "download";

    fn validate_section(&self) -> Result<(), String> {
        if !Self::MAX_PARALLEL_DOWNLOADS_RANGE.contains(&self.max_parallel_downloads) {
            return Err("max_parallel_downloads must be between 1 and 64".into());
        }
        if !Self::MAX_ATTEMPTS_RANGE.contains(&self.max_attempts) {
            return Err("max_attempts must be between 1 and 20".into());
        }
        if !Self::RETRY_INITIAL_DELAY_SECS_RANGE.contains(&self.retry_initial_delay_secs) {
            return Err("retry_initial_delay_secs must be between 0 and 3600".into());
        }
        if !Self::RETRY_BACKOFF_MULTIPLIER_RANGE.contains(&self.retry_backoff_multiplier) {
            return Err("retry_backoff_multiplier must be between 2 and 10".into());
        }
        if !Self::RETRY_MAX_DELAY_SECS_RANGE.contains(&self.retry_max_delay_secs) {
            return Err("retry_max_delay_secs must be between 1 and 86400".into());
        }
        if self.retry_max_delay_secs < self.retry_initial_delay_secs {
            return Err("retry_max_delay_secs must not be below retry_initial_delay_secs".into());
        }
        Ok(())
    }
}

/// Registers the application-wide network download policy. Plugins that can
/// initiate a download call [`ensure_download_settings`] during composition;
/// the resource and persistence registration remain owned here.
pub struct DownloadSettingsPlugin;

impl Plugin for DownloadSettingsPlugin {
    fn build(&self, app: &mut App) {
        app.register_settings_section::<DownloadSettings>();
    }
}

/// Ensure the shared download policy is installed in a composed Bevy app.
///
/// Several independently useful plugins can initiate downloads. Bevy rejects
/// adding the same plugin type twice, so each such plugin calls this one
/// composition helper instead of owning a second registration path. The
/// resource, section key, and persistence remain owned by this crate.
pub fn ensure_download_settings(app: &mut App) {
    if !app.is_plugin_added::<DownloadSettingsPlugin>() {
        app.add_plugins(DownloadSettingsPlugin);
    }
}

#[cfg(test)]
mod download_settings_tests {
    use super::*;

    #[test]
    fn default_download_policy_is_exponential_and_capped() {
        let settings = DownloadSettings::default();
        assert_eq!(settings.max_attempts, 5);
        assert_eq!(settings.retry_delay(1), std::time::Duration::from_secs(1));
        assert_eq!(settings.retry_delay(2), std::time::Duration::from_secs(2));
        assert_eq!(settings.retry_delay(3), std::time::Duration::from_secs(4));
        assert_eq!(settings.retry_delay(4), std::time::Duration::from_secs(8));
        assert_eq!(settings.retry_delay(10), std::time::Duration::from_secs(60));
        assert!(settings.validate_section().is_ok());
    }

    #[test]
    fn download_section_without_policy_fields_uses_defaults() {
        let settings: DownloadSettings = serde_json::from_str(r#"{"max_parallel_downloads": 7}"#)
            .expect("omitted policy fields use documented defaults");
        assert_eq!(settings.max_parallel_downloads, 7);
        assert_eq!(
            settings.max_attempts,
            DownloadSettings::default().max_attempts
        );
        assert_eq!(settings.retry_backoff_multiplier, 2);
    }

    #[test]
    fn invalid_download_policy_is_rejected() {
        let mut settings = DownloadSettings::default();
        settings.max_attempts = 0;
        assert!(settings.validate_section().is_err());
        settings = DownloadSettings::default();
        settings.retry_backoff_multiplier = 1;
        assert!(settings.validate_section().is_err());
    }

    #[test]
    fn composed_downloaders_share_one_settings_plugin() {
        let mut app = App::new();
        ensure_download_settings(&mut app);
        ensure_download_settings(&mut app);
        assert!(app.is_plugin_added::<DownloadSettingsPlugin>());
        assert_eq!(
            app.world().resource::<DownloadSettings>(),
            &DownloadSettings::default()
        );
    }
}

#[cfg(test)]
mod disk_guard_tests {
    use super::*;

    #[derive(Resource, Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
    #[serde(deny_unknown_fields)]
    struct TestSection {
        enabled: bool,
    }

    impl SettingsSection for TestSection {
        const KEY: &'static str = "test_section";
    }

    #[derive(Resource, Serialize, Deserialize, Clone, PartialEq, Debug)]
    struct ValidatedTestSection {
        value: u32,
    }

    impl Default for ValidatedTestSection {
        fn default() -> Self {
            Self { value: 1 }
        }
    }

    impl SettingsSection for ValidatedTestSection {
        const KEY: &'static str = "validated_test_section";

        fn validate_section(&self) -> Result<(), String> {
            (self.value > 0)
                .then_some(())
                .ok_or_else(|| "value must be greater than zero".to_string())
        }
    }

    #[test]
    fn invalid_section_is_removed_and_current_defaults_are_registered() {
        let mut app = App::new();
        app.insert_resource(Settings {
            raw: BTreeMap::from([(
                TestSection::KEY.to_string(),
                serde_json::json!({ "enabled": true, "obsolete": 1 }),
            )]),
            dirty: false,
        });

        app.register_settings_section::<TestSection>();

        assert_eq!(
            app.world().resource::<TestSection>().enabled,
            TestSection::default().enabled
        );
        let settings = app.world().resource::<Settings>();
        assert!(settings.raw(TestSection::KEY).is_none());
        assert!(settings.raw("test_section.bad").is_none());
        assert!(settings.dirty);
    }

    #[test]
    fn semantically_invalid_section_is_removed_and_defaults_are_registered() {
        let mut app = App::new();
        app.insert_resource(Settings {
            raw: BTreeMap::from([(
                ValidatedTestSection::KEY.to_string(),
                serde_json::json!({ "value": 0 }),
            )]),
            dirty: false,
        });

        app.register_settings_section::<ValidatedTestSection>();

        assert_eq!(
            app.world().resource::<ValidatedTestSection>().value,
            ValidatedTestSection::default().value
        );
        let settings = app.world().resource::<Settings>();
        assert!(settings.raw(ValidatedTestSection::KEY).is_none());
        assert!(settings.dirty);
    }

    #[test]
    fn invalid_runtime_section_does_not_replace_last_valid_persisted_value() {
        let mut app = App::new();
        app.insert_resource(Settings {
            raw: BTreeMap::from([(
                ValidatedTestSection::KEY.to_string(),
                serde_json::json!({ "value": 3 }),
            )]),
            dirty: false,
        });
        app.register_settings_section::<ValidatedTestSection>();

        app.world_mut().resource_mut::<ValidatedTestSection>().value = 0;
        app.update();
        assert_eq!(
            app.world()
                .resource::<Settings>()
                .raw(ValidatedTestSection::KEY),
            Some(&serde_json::json!({ "value": 3 }))
        );

        app.world_mut().resource_mut::<ValidatedTestSection>().value = 4;
        app.update();
        assert_eq!(
            app.world()
                .resource::<Settings>()
                .raw(ValidatedTestSection::KEY),
            Some(&serde_json::json!({ "value": 4 }))
        );
    }

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
             real settings file"
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
            assert!(
                !disk_backed(),
                "a test binary must never read or write the real settings"
            );
        }
    }

    /// The opt-out must beat the opt-in. A harness that declared itself ephemeral is
    /// making a safety claim; an ambient `LUNCOSIM_CONFIG` in the surrounding shell
    /// must not quietly re-open the developer's real settings file underneath it.
    ///
    /// Serial by construction: it mutates process environment, so it restores both
    /// variables before returning.
    #[test]
    fn ephemeral_beats_an_explicit_config_dir() {
        let prev_cfg = std::env::var_os("LUNCOSIM_CONFIG");
        let prev_eph = std::env::var_os(EPHEMERAL_SETTINGS_VAR);

        unsafe { std::env::set_var("LUNCOSIM_CONFIG", "/nonexistent/should-not-be-read") };
        assert!(
            disk_backed(),
            "an explicit config dir must still opt a test process back in"
        );

        use_ephemeral_settings();
        assert!(
            !disk_backed(),
            "`use_ephemeral_settings` must win over LUNCOSIM_CONFIG — this is the guard \
             that keeps `scene_test` from persisting CelestialCadenceSettings::EXACT into \
             the developer's real settings.json"
        );

        unsafe {
            match prev_cfg {
                Some(v) => std::env::set_var("LUNCOSIM_CONFIG", v),
                None => std::env::remove_var("LUNCOSIM_CONFIG"),
            }
            match prev_eph {
                Some(v) => std::env::set_var(EPHEMERAL_SETTINGS_VAR, v),
                None => std::env::remove_var(EPHEMERAL_SETTINGS_VAR),
            }
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
        s.raw
            .insert("telemetry".into(), serde_json::json!({ "enabled": false }));
        s.dirty = true;
        s.write_if_dirty();
        assert!(
            !s.dirty,
            "the dirty bit must clear so we don't retry the suppressed write"
        );
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
