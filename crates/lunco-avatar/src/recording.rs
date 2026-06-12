//! Screen recording: hotkey + persisted settings + command surface over
//! Bevy 0.18's built-in [`EasyScreenRecordPlugin`] (h264 / x264 encoder).
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
//! # Filenames & output folder (built-in encoder constraints)
//!
//! Bevy 0.18.1's recorder hard-codes the output to `<window-title>-<epoch_ms>.h264`
//! (a raw H.264 elementary stream) written to the **process working directory** —
//! the `output_dir` field only exists on bevy `main`, not 0.18.1. Our window
//! title is the binary name (`sandbox`), so files come out `sandbox-<ms>.h264`,
//! i.e. "binary + timestamp".
//!
//! To honour [`RecordingSettings::output_dir`] anyway, on stop we spawn a small
//! worker thread (`encoder::spawn_relocator`) that waits for the encoder's own
//! background flush to finish (file size goes stable), then relocates the new
//! `.h264` into the configured folder under a **sanitized** name — applying the
//! `overwrite` flag. It runs off the Bevy loop on purpose: the app's winit loop
//! can go idle right after the stop event, so a poll-in-`Update` finalizer would
//! stall until something else ticked the loop. Best-effort and time-boxed: on
//! timeout the file is left in the working directory (logged), never blocking.
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
pub fn on_toggle_recording(_cmd: ToggleRecording, mut state: ResMut<RecordingState>) {
    let next = !state.active;
    set_recording(&mut state, next);
}

#[on_command(StartRecording)]
pub fn on_start_recording(_cmd: StartRecording, mut state: ResMut<RecordingState>) {
    set_recording(&mut state, true);
}

#[on_command(StopRecording)]
pub fn on_stop_recording(_cmd: StopRecording, mut state: ResMut<RecordingState>) {
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

// ─── Encoder bridge + file relocation (feature-gated) ────────────────────────

/// Everything that touches the `bevy_dev_tools` encoder. Compiled only with the
/// `recording` feature (which is native-/non-Windows-only — that's where the
/// `RecordScreen` message type and x264 exist).
#[cfg(feature = "recording")]
mod encoder {
    use super::*;
    use std::collections::HashSet;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    /// Holds the `.h264` set present in the working dir when recording started,
    /// so the relocator can spot the new file (set difference). The built-in
    /// 0.18.1 encoder writes there with no folder control — see the module docs.
    #[derive(Resource, Default)]
    pub(super) struct RecorderIo {
        snapshot: HashSet<OsString>,
    }

    /// `.h264` files currently in `dir`.
    fn h264_in(dir: &Path) -> HashSet<OsString> {
        let mut set = HashSet::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            for entry in rd.flatten() {
                if entry.path().extension().and_then(|e| e.to_str()) == Some("h264") {
                    set.insert(entry.file_name());
                }
            }
        }
        set
    }

    /// Bridge [`RecordingState`] changes → `RecordScreen` messages. On start,
    /// snapshot the working dir's `.h264` files; on stop, hand off relocation to
    /// a background thread. Skips the initial `active == false` observation so
    /// startup doesn't emit a spurious `Stop`.
    pub(super) fn drive_screen_record(
        state: Res<RecordingState>,
        settings: Res<RecordingSettings>,
        mut io: ResMut<RecorderIo>,
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
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        if state.active {
            io.snapshot = h264_in(&cwd);
            writer.write(bevy_dev_tools::RecordScreen::Start);
        } else {
            writer.write(bevy_dev_tools::RecordScreen::Stop);
            // Relocate OFF the Bevy loop: the encoder flushes the file on its own
            // background thread, and the app's winit loop may go idle right after
            // the stop event (reactive mode / occluded window) — so a poll-in-
            // `Update` finalizer would stall until something else ticked the loop
            // (observed in testing). A dedicated thread is independent of that.
            spawn_relocator(
                std::mem::take(&mut io.snapshot),
                cwd,
                settings.resolved_output_dir(),
                settings.overwrite,
            );
        }
    }

    /// Wait (off the Bevy loop) for the encoder's new `.h264` to appear in `cwd`
    /// and finish flushing — its size stable for ~0.6 s — then move it into
    /// `out_dir`. Time-boxed; on timeout the file is left in `cwd` (logged).
    ///
    /// Pure std + a worker thread: deliberately bypasses the `disallowed_methods`
    /// lint (raw `std::fs`/`std::thread`), which exists to keep wasm-reachable
    /// code portable. This whole module is `#[cfg(feature = "recording")]`, which
    /// is native-/non-Windows-only and never compiled for wasm.
    #[allow(clippy::disallowed_methods)]
    fn spawn_relocator(
        snapshot: HashSet<OsString>,
        cwd: PathBuf,
        out_dir: PathBuf,
        overwrite: bool,
    ) {
        use std::time::Duration;
        const STEP: Duration = Duration::from_millis(200);
        const STEP_MS: u64 = 200;
        const FIND_TIMEOUT_MS: u64 = 15_000;
        const STABLE_MS: u64 = 600;
        const TOTAL_TIMEOUT_MS: u64 = 30_000;

        std::thread::spawn(move || {
            // 1. Find the file the encoder created (not present at start).
            let mut waited = 0u64;
            let name = loop {
                if let Some(n) = h264_in(&cwd).difference(&snapshot).next().cloned() {
                    break n;
                }
                if waited >= FIND_TIMEOUT_MS {
                    warn!("[recording] no .h264 appeared after stop; gave up");
                    return;
                }
                std::thread::sleep(STEP);
                waited += STEP_MS;
            };

            // 2. Wait for the encoder's flush to finish (size stable).
            let src = cwd.join(&name);
            let mut last_size = 0u64;
            let mut stable = 0u64;
            let mut total = waited;
            loop {
                let size = std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0);
                if size > 0 && size == last_size {
                    stable += STEP_MS;
                } else {
                    stable = 0;
                    last_size = size;
                }
                if stable >= STABLE_MS {
                    break;
                }
                if total >= TOTAL_TIMEOUT_MS {
                    warn!("[recording] {} never stabilized; left in working dir", src.display());
                    return;
                }
                std::thread::sleep(STEP);
                total += STEP_MS;
            }

            // 3. Move it into the configured folder.
            relocate(&src, &out_dir, overwrite, &name);
        });
    }

    /// Move `src` into `dir` under a shell-safe name, honouring `overwrite`.
    /// Falls back to copy+remove across filesystems. Logs the final path and a
    /// quoted ffmpeg remux hint. Runs on the relocator thread (see
    /// `spawn_relocator` for the `disallowed_methods` rationale).
    #[allow(clippy::disallowed_methods)]
    fn relocate(src: &Path, dir: &Path, overwrite: bool, name: &OsString) {
        if let Err(e) = std::fs::create_dir_all(dir) {
            warn!(
                "[recording] mkdir {} failed: {e}; left {} in working dir",
                dir.display(),
                src.display()
            );
            return;
        }
        // The built-in encoder names the file after the window title, which can
        // contain spaces / em-dashes (e.g. "sandbox — Listening on 4011-…"):
        // ugly and shell-hostile. Sanitize to `<safe>-<ms>.h264`.
        let safe = sanitize_filename(name);
        let mut dest = dir.join(&safe);
        if dest.exists() && !overwrite {
            let stem = dest
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("recording")
                .to_string();
            dest = dir.join(format!("{stem}-1.h264"));
        }
        let moved = std::fs::rename(src, &dest).is_ok()
            || (std::fs::copy(src, &dest).is_ok() && std::fs::remove_file(src).is_ok());
        if moved {
            info!("[recording] saved {}", dest.display());
            info!(
                "[recording] raw H.264 — remux: ffmpeg -framerate 30 -i '{}' -c copy '{}.mp4'",
                dest.display(),
                dest.with_extension("").display()
            );
        } else {
            warn!(
                "[recording] could not move {} → {}; left in working dir",
                src.display(),
                dest.display()
            );
        }
    }

    #[cfg(test)]
    mod enc_tests {
        #[test]
        fn sanitizes_window_title_name() {
            let n = std::ffi::OsString::from("sandbox — Listening on 4011-1781233931122.h264");
            assert_eq!(
                super::sanitize_filename(&n),
                "sandbox-Listening-on-4011-1781233931122.h264"
            );
        }

        #[test]
        fn sanitize_trims_and_collapses() {
            let n = std::ffi::OsString::from("  weird   name!!.h264 ");
            assert_eq!(super::sanitize_filename(&n), "weird-name-.h264");
        }
    }

    /// Make a filename shell-safe: keep `[A-Za-z0-9._]`, collapse every other
    /// run (spaces, em-dashes, hyphens, …) into a single `-`, and trim leading/
    /// trailing `-`. Empty result falls back to `recording.h264`.
    fn sanitize_filename(name: &OsString) -> String {
        let raw = name.to_string_lossy();
        let mut out = String::with_capacity(raw.len());
        let mut prev_sep = false;
        for c in raw.chars() {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
                out.push(c);
                prev_sep = false;
            } else if !prev_sep {
                out.push('-');
                prev_sep = true;
            }
        }
        let trimmed = out.trim_matches('-');
        if trimmed.is_empty() {
            "recording.h264".to_string()
        } else {
            trimmed.to_string()
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
        app.add_plugins(bevy_dev_tools::EasyScreenRecordPlugin {
            // We drive recording via Ctrl+Shift+R → RecordScreen messages, so
            // park the plugin's own single-key toggle on a key no keyboard has
            // (its default is Space, which we want to keep free).
            toggle: KeyCode::F24,
            ..default()
        });
        app.init_resource::<encoder::RecorderIo>();
        app.add_systems(Update, encoder::drive_screen_record);
        let dir = lunco_settings::load_section_from_disk::<RecordingSettings>()
            .resolved_output_dir();
        info!(
            "[recording] encoder enabled (Ctrl+Shift+R). bevy 0.18.1 writes \
             <title>-<ms>.h264 to the working dir; finished files are moved to {}.",
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
