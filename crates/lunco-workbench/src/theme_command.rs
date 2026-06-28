//! Theme mode persistence + the `SetTheme` API command.
//!
//! Persists `ThemeMode` to `~/.lunco/settings.json` under the
//! `"theme"` key (via `lunco-settings`) so a Light-mode user doesn't
//! get blasted with Dark on every relaunch. The command mirrors what
//! the `Settings ▸ Theme` toggle does, but reachable from the
//! HTTP/script bus so test loops can flip Light/Dark and screenshot
//! both modes without driving the GUI.

use bevy::prelude::*;
use lunco_core::{Command, on_command, register_commands};
use lunco_settings::{AppSettingsExt, SettingsSection};
use lunco_theme::{Theme, ThemeMode};
use serde::{Deserialize, Serialize};

/// Persisted user preference for the active theme. Stored under the
/// `"theme"` key of `settings.json`.
#[derive(Resource, Serialize, Deserialize, Default, Clone, Copy, PartialEq, Debug)]
pub struct ThemePreference {
    /// Currently active theme mode (Light / Dark / System / etc.).
    pub mode: ThemeMode,
}

impl SettingsSection for ThemePreference {
    const KEY: &'static str = "theme";
}

/// Set or toggle the active theme mode. Omit `mode` to toggle.
#[Command(default)]
pub struct SetTheme {
    /// `"dark"` / `"light"` (case-insensitive). When `None`, toggles.
    pub mode: Option<String>,
}

#[on_command(SetTheme)]
fn on_set_theme(
    trigger: On<SetTheme>,
    mut theme: ResMut<Theme>,
    mut pref: ResMut<ThemePreference>,
) {
    let target = trigger.event().mode.as_deref().map(|s| s.to_ascii_lowercase());
    let explicit = match target.as_deref() {
        Some("dark") => Some(ThemeMode::Dark),
        Some("light") => Some(ThemeMode::Light),
        _ => None,
    };
    match explicit {
        // CQ-519: apply the requested mode directly instead of toggling
        // (a toggle only lands on the right value for a 2-state enum).
        Some(m) => theme.set_mode(m),
        None => theme.toggle_mode(),
    }
    if pref.mode != theme.mode {
        pref.mode = theme.mode;
    }
}

/// Settings menu's `Theme` button also flips the mode directly on
/// `Theme` (see `lib.rs`). Mirror that change into the persisted
/// preference so closing and reopening the app remembers it.
fn sync_pref_from_theme(theme: Res<Theme>, mut pref: ResMut<ThemePreference>) {
    if theme.is_changed() && pref.mode != theme.mode {
        pref.mode = theme.mode;
    }
}

/// On first frame, apply the persisted preference to the live
/// `Theme` resource. Idempotent: if the preference matches the
/// already-loaded `Theme` (default Dark on first run), this is a
/// no-op.
fn apply_pref_to_theme(pref: Res<ThemePreference>, mut theme: ResMut<Theme>) {
    theme.set_mode(pref.mode);
}

register_commands!(on_set_theme,);

/// Plugin registering the [`ThemePreference`] settings section and the
/// [`SetTheme`] command observer.
pub struct ThemeCommandPlugin;

impl Plugin for ThemeCommandPlugin {
    fn build(&self, app: &mut App) {
        app.register_settings_section::<ThemePreference>();
        // Apply persisted preference once at startup, then keep
        // preference in sync with any subsequent theme changes
        // (menu toggle, command, programmatic).
        app.add_systems(Startup, apply_pref_to_theme);
        app.add_systems(Update, sync_pref_from_theme);
        register_all_commands(app);
    }
}
