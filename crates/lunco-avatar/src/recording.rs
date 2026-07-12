//! Screen recording: hotkey + persisted settings + command surface over
//! Bevy's built-in [`EasyScreenRecordPlugin`] (h264 / x264 encoder).
//!
//! # What's always compiled
//!
//! The *control surface* — [`RecordingSettings`] (a `settings.json` section),
//! the [`RecordingState`] resource, the `ToggleRecording`/`StartRecording`/
//! `StopRecording` commands, and the Ctrl+Shift+R hotkey — is always built, so
//! the UI/API can offer recording controls regardless of platform.
//!
//! # What's feature-gated (`recording`)
//!
//! The actual encoder ([`EasyScreenRecordPlugin`] + the `RecordScreen` message
//! bridge) is gated behind the crate's `recording` cargo feature, which pulls
//! `bevy_dev_tools` with its `screenrecording` feature (links **libx264**).
//! That encoder is **native-only and not supported on Windows** — build with
//! `--features recording` on Linux/macOS. Without the feature, toggling logs a
//! warning and records nothing.
//!
//! # Filenames & output folder
//!
//! The recorder writes `<window-title>-<epoch_ms>.h264` (a raw H.264 elementary
//! stream) into the plugin's `output_dir`, which we set from
//! [`RecordingSettings::output_dir`] at startup (Bevy ≥0.19; on 0.18.1 this field
//! did not exist and files landed in the working directory, which required a
//! post-stop relocator thread — since removed). The dir is resolved once when the
//! plugin is built; changing `output_dir` in settings takes effect on the next
//! launch. The [`RecordingSettings::overwrite`] flag is advisory — the encoder
//! timestamps filenames, so collisions don't occur.
//!
//! `.h264` is a raw stream; remux to mp4 with
//! `ffmpeg -framerate 30 -i file.h264 -c copy file.mp4`.

use std::path::PathBuf;

use bevy::prelude::*;
use lunco_core::{on_command, Command};
use lunco_settings::{AppSettingsExt, SettingsSection};
use serde::{Deserialize, Serialize};

// ─── Settings ────────────────────────────────────────────────────────────────

/// Persisted recording preferences. Stored under `settings.json` → `"recording"`.
#[derive(Resource, Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct RecordingSettings {
    /// Directory videos are written to. Empty string → the OS Videos folder
    /// (`$XDG_VIDEOS_DIR` or `~/Videos`), see [`Self::resolved_output_dir`].
    pub output_dir: String,
    /// Hotkey chord that toggles recording, e.g. `"ctrl+shift+r"`.
    /// Parsed as `[ctrl+][shift+][alt+]<key>` where `<key>` is a letter
    /// (`a`–`z`), digit (`0`–`9`), `f1`–`f12`, or `space`.
    pub hotkey: String,
    /// Overwrite an existing file of the same name.
    ///
    /// Advisory only with the built-in encoder: it timestamps filenames so
    /// collisions don't occur. Reserved for a future custom-filename backend.
    pub overwrite: bool,
}

impl Default for RecordingSettings {
    fn default() -> Self {
        Self {
            output_dir: String::new(),
            hotkey: "ctrl+shift+r".to_string(),
            overwrite: false,
        }
    }
}

impl SettingsSection for RecordingSettings {
    const KEY: &'static str = "recording";
}

impl RecordingSettings {
    /// Folder recordings are written to. Blank [`Self::output_dir`] resolves to
    /// the OS Videos directory.
    pub fn resolved_output_dir(&self) -> PathBuf {
        if !self.output_dir.trim().is_empty() {
            return PathBuf::from(self.output_dir.trim());
        }
        default_videos_dir()
    }
}

/// The user's Videos folder: `$XDG_VIDEOS_DIR` if set, else `~/Videos`, else
/// (no `HOME`, e.g. odd headless envs) the app data dir. Where a media tool
/// should drop recordings by default — not a buried app-data path.
fn default_videos_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_VIDEOS_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home).join("Videos");
        }
    }
    lunco_assets::user_data_dir().join("recordings")
}

// ─── State ───────────────────────────────────────────────────────────────────

/// Whether a recording is currently in progress. Drives the `RecordScreen`
/// bridge (under the `recording` feature) and any UI "● REC" indicator.
#[derive(Resource, Default, Reflect, Debug)]
#[reflect(Resource)]
pub struct RecordingState {
    pub active: bool,
}

// ─── Commands ────────────────────────────────────────────────────────────────

/// Toggle recording on/off. Bound to the configured hotkey (Ctrl+Shift+R).
#[Command(default)]
pub struct ToggleRecording {}

/// Begin recording (idempotent — no-op if already recording).
#[Command(default)]
pub struct StartRecording {}

/// Stop recording (idempotent — no-op if not recording).
#[Command(default)]
pub struct StopRecording {}

#[on_command(ToggleRecording)]
pub fn on_toggle_recording(trigger: On<ToggleRecording>, mut state: ResMut<RecordingState>) {
    let next = !state.active;
    set_recording(&mut state, next);
}

#[on_command(StartRecording)]
pub fn on_start_recording(trigger: On<StartRecording>, mut state: ResMut<RecordingState>) {
    set_recording(&mut state, true);
}

#[on_command(StopRecording)]
pub fn on_stop_recording(trigger: On<StopRecording>, mut state: ResMut<RecordingState>) {
    set_recording(&mut state, false);
}

/// Apply a desired recording state, logging the transition. Change-detection on
/// `RecordingState` (the `Res::is_changed` the bridge relies on) only fires when
/// the value actually flips, so we early-return on a no-op.
fn set_recording(state: &mut RecordingState, active: bool) {
    if state.active == active {
        return;
    }
    state.active = active;
    if cfg!(feature = "recording") {
        info!("[recording] {}", if active { "started" } else { "stopped" });
    } else {
        warn!(
            "[recording] toggle to {active} ignored — this binary was built \
             without the `recording` feature (no encoder). Rebuild native with \
             `--features recording`."
        );
    }
}

// ─── Hotkey ──────────────────────────────────────────────────────────────────

/// A parsed keyboard chord: required modifier state + the triggering key.
#[derive(Clone, PartialEq, Debug)]
struct Chord {
    ctrl: bool,
    shift: bool,
    alt: bool,
    key: KeyCode,
}

impl Chord {
    /// True the frame `key` is freshly pressed with exactly the required
    /// modifier state held.
    fn just_triggered(&self, keys: &ButtonInput<KeyCode>) -> bool {
        let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
        let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let alt = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
        ctrl == self.ctrl && shift == self.shift && alt == self.alt && keys.just_pressed(self.key)
    }
}

/// Parse `"ctrl+shift+r"`-style strings into a [`Chord`]. Returns `None` if no
/// non-modifier key is present or the key token is unrecognised.
fn parse_chord(spec: &str) -> Option<Chord> {
    let mut chord = Chord {
        ctrl: false,
        shift: false,
        alt: false,
        key: KeyCode::KeyR,
    };
    let mut have_key = false;
    for token in spec.split('+') {
        match token.trim().to_ascii_lowercase().as_str() {
            "" => {}
            "ctrl" | "control" | "cmd" | "super" | "meta" => chord.ctrl = true,
            "shift" => chord.shift = true,
            "alt" | "option" => chord.alt = true,
            other => {
                chord.key = key_from_str(other)?;
                have_key = true;
            }
        }
    }
    have_key.then_some(chord)
}

/// Map a lowercase key token to its [`KeyCode`]. Covers letters, digits,
/// `f1`–`f12`, and `space` — enough for realistic recording rebinds.
fn key_from_str(token: &str) -> Option<KeyCode> {
    Some(match token {
        "a" => KeyCode::KeyA,
        "b" => KeyCode::KeyB,
        "c" => KeyCode::KeyC,
        "d" => KeyCode::KeyD,
        "e" => KeyCode::KeyE,
        "f" => KeyCode::KeyF,
        "g" => KeyCode::KeyG,
        "h" => KeyCode::KeyH,
        "i" => KeyCode::KeyI,
        "j" => KeyCode::KeyJ,
        "k" => KeyCode::KeyK,
        "l" => KeyCode::KeyL,
        "m" => KeyCode::KeyM,
        "n" => KeyCode::KeyN,
        "o" => KeyCode::KeyO,
        "p" => KeyCode::KeyP,
        "q" => KeyCode::KeyQ,
        "r" => KeyCode::KeyR,
        "s" => KeyCode::KeyS,
        "t" => KeyCode::KeyT,
        "u" => KeyCode::KeyU,
        "v" => KeyCode::KeyV,
        "w" => KeyCode::KeyW,
        "x" => KeyCode::KeyX,
        "y" => KeyCode::KeyY,
        "z" => KeyCode::KeyZ,
        "0" => KeyCode::Digit0,
        "1" => KeyCode::Digit1,
        "2" => KeyCode::Digit2,
        "3" => KeyCode::Digit3,
        "4" => KeyCode::Digit4,
        "5" => KeyCode::Digit5,
        "6" => KeyCode::Digit6,
        "7" => KeyCode::Digit7,
        "8" => KeyCode::Digit8,
        "9" => KeyCode::Digit9,
        "f1" => KeyCode::F1,
        "f2" => KeyCode::F2,
        "f3" => KeyCode::F3,
        "f4" => KeyCode::F4,
        "f5" => KeyCode::F5,
        "f6" => KeyCode::F6,
        "f7" => KeyCode::F7,
        "f8" => KeyCode::F8,
        "f9" => KeyCode::F9,
        "f10" => KeyCode::F10,
        "f11" => KeyCode::F11,
        "f12" => KeyCode::F12,
        "space" => KeyCode::Space,
        _ => return None,
    })
}

/// Reads the keyboard each frame and fires [`ToggleRecording`] when the
/// configured chord triggers. The chord string is re-parsed only when the
/// setting changes (cached in a `Local`), so steady-state is a cheap modifier
/// compare — no per-frame string parsing.
fn recording_hotkey_input(
    keys: Res<ButtonInput<KeyCode>>,
    settings: Res<RecordingSettings>,
    mut cached: Local<Option<(String, Option<Chord>)>>,
    mut commands: Commands,
) {
    let stale = match &*cached {
        Some((spec, _)) => spec != &settings.hotkey,
        None => true,
    };
    if stale {
        *cached = Some((settings.hotkey.clone(), parse_chord(&settings.hotkey)));
    }
    if let Some((_, Some(chord))) = &*cached {
        if chord.just_triggered(&keys) {
            commands.trigger(ToggleRecording {});
        }
    }
}

// ─── Encoder bridge (feature-gated) ──────────────────────────────────────────

/// Everything that touches the `bevy_dev_tools` encoder. Compiled only with the
/// `recording` feature (which is native-/non-Windows-only — that's where the
/// `RecordScreen` message type and x264 exist).
#[cfg(feature = "recording")]
mod encoder {
    use super::*;

    /// Bridge [`RecordingState`] changes → `RecordScreen` messages so our
    /// Ctrl+Shift+R hotkey drives the encoder (its own single-key toggle is
    /// parked on F24). Skips the initial `active == false` observation so startup
    /// doesn't emit a spurious `Stop`. The encoder writes finished files straight
    /// into the plugin's `output_dir` (set in [`build_recording`]).
    pub(super) fn drive_screen_record(
        state: Res<RecordingState>,
        mut last: Local<Option<bool>>,
        mut writer: bevy::prelude::MessageWriter<bevy_dev_tools::RecordScreen>,
    ) {
        if *last == Some(state.active) {
            return;
        }
        if last.is_none() && !state.active {
            *last = Some(false);
            return;
        }
        *last = Some(state.active);
        if state.active {
            writer.write(bevy_dev_tools::RecordScreen::Start);
        } else {
            writer.write(bevy_dev_tools::RecordScreen::Stop);
        }
    }
}

// ─── Wiring ──────────────────────────────────────────────────────────────────

/// Wire the recording control surface (and, under the `recording` feature, the
/// encoder) into the app. Called from `LunCoAvatarPlugin::build`. Command
/// observers are registered separately via the crate's `register_commands!`.
pub fn build_recording(app: &mut App) {
    app.register_settings_section::<RecordingSettings>();
    app.init_resource::<RecordingState>();
    app.register_type::<RecordingState>();
    app.add_systems(Update, recording_hotkey_input);

    #[cfg(feature = "recording")]
    {
        let dir = lunco_settings::load_section_from_disk::<RecordingSettings>()
            .resolved_output_dir();
        app.add_plugins(bevy_dev_tools::EasyScreenRecordPlugin {
            // We drive recording via Ctrl+Shift+R → RecordScreen messages, so
            // park the plugin's own single-key toggle on a key no keyboard has
            // (its default is Space, which we want to keep free).
            toggle: KeyCode::F24,
            // Native output folder (Bevy ≥0.19). Resolved once at startup.
            output_dir: Some(dir.clone()),
            ..default()
        });
        app.add_systems(Update, encoder::drive_screen_record);
        info!(
            "[recording] encoder enabled (Ctrl+Shift+R). Writes \
             <title>-<ms>.h264 to {}.",
            dir.display()
        );
    }
    #[cfg(not(feature = "recording"))]
    info!(
        "[recording] control surface ready (no encoder; build native with \
         `--features recording` to capture video)"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_chord() {
        let c = parse_chord("ctrl+shift+r").unwrap();
        assert_eq!(
            c,
            Chord { ctrl: true, shift: true, alt: false, key: KeyCode::KeyR }
        );
    }

    #[test]
    fn parse_is_case_and_space_insensitive() {
        let c = parse_chord(" CTRL + Shift + R ").unwrap();
        assert!(c.ctrl && c.shift && !c.alt);
        assert_eq!(c.key, KeyCode::KeyR);
    }

    #[test]
    fn bare_key_has_no_modifiers() {
        let c = parse_chord("f9").unwrap();
        assert_eq!(
            c,
            Chord { ctrl: false, shift: false, alt: false, key: KeyCode::F9 }
        );
    }

    #[test]
    fn rejects_modifier_only_or_unknown_key() {
        assert!(parse_chord("ctrl+shift").is_none());
        assert!(parse_chord("ctrl+plus").is_none());
        assert!(parse_chord("").is_none());
    }

    #[test]
    fn output_dir_override_and_default() {
        // Explicit override wins verbatim.
        let custom = RecordingSettings {
            output_dir: "/tmp/vids".into(),
            ..Default::default()
        };
        assert_eq!(custom.resolved_output_dir(), PathBuf::from("/tmp/vids"));
        // Blank → an absolute OS Videos path (~/Videos when HOME is set).
        let def = RecordingSettings::default().resolved_output_dir();
        assert!(def.is_absolute());
        if std::env::var("HOME").is_ok() && std::env::var("XDG_VIDEOS_DIR").is_err() {
            assert!(def.ends_with("Videos"));
        }
    }
}
