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

/// A theme mode applied WITHOUT touching the persisted preference — a film
/// scene declaring "this take records dark" must not rewrite the operator's
/// editor preference. While the live `Theme` equals this override, the
/// pref-sync below stands down; the moment anything ELSE changes the theme
/// (menu toggle, a persisting `SetTheme`), the override clears and normal
/// persistence resumes.
///
/// This is deliberately the seam for the planned USD-schema theme
/// (`lunco:ui:*` authored on a scene prim): scene-authored appearance should
/// land as a non-persisted override through this same resource, so the
/// command path and the future declarative path share one mechanism.
#[derive(Resource, Default)]
pub struct NonPersistedThemeOverride(pub Option<ThemeMode>);

/// Set or toggle the active theme mode. Omit `mode` to toggle.
#[Command(default)]
pub struct SetTheme {
    /// `"dark"` / `"light"` (case-insensitive). When `None`, toggles.
    pub mode: Option<String>,
    /// `false` = apply for this session only, leave `settings.json` alone.
    /// Default `true` (the historical behavior).
    pub persist: Option<bool>,
}

#[on_command(SetTheme)]
fn on_set_theme(
    trigger: On<SetTheme>,
    mut theme: ResMut<Theme>,
    mut pref: ResMut<ThemePreference>,
    mut override_mode: ResMut<NonPersistedThemeOverride>,
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
    if trigger.event().persist == Some(false) {
        override_mode.0 = Some(theme.mode);
        return;
    }
    override_mode.0 = None;
    if pref.mode != theme.mode {
        pref.mode = theme.mode;
    }
}

/// Settings menu's `Theme` button also flips the mode directly on
/// `Theme` (see `lib.rs`). Mirror that change into the persisted
/// preference so closing and reopening the app remembers it — unless the
/// current mode is a scene's non-persisted override, which must not leak
/// into the operator's saved settings.
fn sync_pref_from_theme(
    theme: Res<Theme>,
    mut pref: ResMut<ThemePreference>,
    mut override_mode: ResMut<NonPersistedThemeOverride>,
) {
    if !theme.is_changed() {
        return;
    }
    if override_mode.0 == Some(theme.mode) {
        return;
    }
    // Any change AWAY from the override ends it.
    override_mode.0 = None;
    if pref.mode != theme.mode {
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
        app.init_resource::<NonPersistedThemeOverride>();
        app.register_settings_section::<ThemePreference>();
        // Apply persisted preference once at startup, then keep
        // preference in sync with any subsequent theme changes
        // (menu toggle, command, programmatic).
        app.add_systems(Startup, apply_pref_to_theme);
        app.add_systems(Update, sync_pref_from_theme);
        register_all_commands(app);
    }
}
