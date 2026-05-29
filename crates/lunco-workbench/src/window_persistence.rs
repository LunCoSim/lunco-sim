//! Persisted OS-window geometry (size / position / maximized).
//!
//! VSCode reopens with the same window bounds it closed with. We do the
//! same: the *global default* geometry rides in the one shared
//! `~/.lunco/settings.json` under the `"window"` key, via
//! [`lunco_settings`] — the canonical home for persisted user
//! preferences (AGENTS.md §3). Per-Twin window bounds layer on top in
//! [`crate::workspace_state`] (VSCode's `workspaceStorage`); this module
//! owns only the global baseline.
//!
//! ## Restore happens before the window exists
//!
//! The primary `Window` is built inside each binary's `WindowPlugin`,
//! *before* `App` runs. So restore can't go through a Bevy system — it
//! reads the section straight off disk with
//! [`lunco_settings::load_section_from_disk`] and feeds the result into
//! the `Window` the binary constructs. [`load_window_geometry`] wraps
//! that (native-only; wasm has no OS window to restore).
//!
//! ## Save happens reactively
//!
//! [`WorkbenchPlugin`](crate::WorkbenchPlugin) registers
//! [`WindowGeometry`] as a settings section (so `lunco-settings` flushes
//! it on change) and adds [`save_window_geometry`], which mirrors the
//! live `Window` back into the resource only when the window actually
//! moved/resized (`Changed<Window>`-gated — no per-frame polling, per
//! AGENTS.md §7.1).

use bevy::prelude::*;
use bevy::window::{
    MonitorSelection, PrimaryWindow, WindowPosition, WindowResolution,
};
use lunco_settings::{AppSettingsExt, SettingsSection};
use serde::{Deserialize, Serialize};

use crate::window_command::WindowMaximized;

/// Registers [`WindowGeometry`] as a settings section (so it loads on
/// startup and `lunco-settings` flushes it on change), restores the
/// maximized state once the window exists, and mirrors live window
/// moves/resizes back into the resource. Native-only: wasm has no OS
/// window to persist. Idempotent.
pub struct WindowPersistencePlugin;

impl Plugin for WindowPersistencePlugin {
    fn build(&self, app: &mut App) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            app.register_settings_section::<WindowGeometry>();
            app.add_systems(Startup, restore_maximized_on_startup);
            app.add_systems(Update, save_window_geometry);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = app;
    }
}

/// Persisted primary-window geometry. Stored under the `"window"` key
/// of `settings.json`. `Default` (all-zero / `None`) means "never saved
/// yet" — callers fall back to their hardcoded ship defaults.
#[derive(Resource, Serialize, Deserialize, Default, Clone, Copy, PartialEq, Debug)]
pub struct WindowGeometry {
    /// Logical width in points. `0.0` ⇒ unset.
    pub width: f32,
    /// Logical height in points. `0.0` ⇒ unset.
    pub height: f32,
    /// Top-left X in physical pixels, if the window had an explicit
    /// position. `None` ⇒ let the binary center it.
    pub x: Option<i32>,
    /// Top-left Y in physical pixels. See [`Self::x`].
    pub y: Option<i32>,
    /// Whether the window was maximized at save time.
    pub maximized: bool,
}

impl SettingsSection for WindowGeometry {
    const KEY: &'static str = "window";
}

/// Ship-default primary-window width (logical points) used the first
/// time an app launches, before any geometry has been persisted. Lives
/// here as a single named constant rather than a magic number copied
/// into each binary (AGENTS.md §3 tunability mandate).
pub const DEFAULT_WINDOW_WIDTH: f32 = 1600.0;
/// Ship-default primary-window height. See [`DEFAULT_WINDOW_WIDTH`].
pub const DEFAULT_WINDOW_HEIGHT: f32 = 1000.0;

impl WindowGeometry {
    /// True when nothing has been persisted yet (so callers fall back to
    /// the ship defaults rather than a degenerate `0x0` window).
    pub fn is_unset(&self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }

    /// Resolution to seed the primary `Window` with — the saved size, or
    /// [`DEFAULT_WINDOW_WIDTH`]×[`DEFAULT_WINDOW_HEIGHT`] when unset.
    pub fn resolution(&self) -> WindowResolution {
        if self.is_unset() {
            WindowResolution::new(DEFAULT_WINDOW_WIDTH as u32, DEFAULT_WINDOW_HEIGHT as u32)
        } else {
            WindowResolution::new(self.width as u32, self.height as u32)
        }
    }

    /// Position to seed the primary `Window` with. Restores the saved
    /// top-left when present; otherwise centers on the primary monitor
    /// (the original ship behaviour).
    pub fn position(&self) -> WindowPosition {
        match (self.x, self.y) {
            (Some(x), Some(y)) if !self.is_unset() => {
                WindowPosition::At(IVec2::new(x, y))
            }
            _ => WindowPosition::Centered(MonitorSelection::Primary),
        }
    }
}

/// Build the primary `Window` with the merged-titlebar chrome
/// ([`merged_titlebar_window`](crate::merged_titlebar_window)) **and**
/// the persisted geometry already applied. This is the one place window
/// defaults + restore live, so binaries don't repeat size magic numbers
/// or restore logic — they just spread their platform-specific extras
/// over the result:
///
/// ```ignore
/// // native:
/// primary_window: Some(Window {
///     present_mode,
///     ..lunco_workbench::restored_window("My App")
/// }),
/// ```
///
/// On wasm there's no OS window to restore, so this is exactly
/// `merged_titlebar_window(title)` (the binary still adds its `canvas`
/// fields).
pub fn restored_window(title: impl Into<String>) -> Window {
    #[allow(unused_mut)]
    let mut window = crate::merged_titlebar_window(title);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let g = load_window_geometry();
        window.resolution = g.resolution();
        window.position = g.position();
    }
    window
}

/// Read the persisted [`WindowGeometry`] from `settings.json` before the
/// `App` is built. [`restored_window`] uses this; exposed for callers
/// that build their `Window` by hand. Returns `Default` (⇒ ship
/// defaults) on wasm or when nothing is saved.
pub fn load_window_geometry() -> WindowGeometry {
    #[cfg(target_arch = "wasm32")]
    {
        WindowGeometry::default()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        lunco_settings::load_section_from_disk::<WindowGeometry>()
    }
}

/// Re-apply a restored `maximized` state once the window exists. Bevy's
/// `Window` can't be created already-maximized portably, so we drive it
/// through the same [`MaximizeWindow`](crate::window_command::MaximizeWindow)
/// command the title-bar uses (it carries the Linux-compositor fallback).
/// Runs once at startup.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn restore_maximized_on_startup(world: &mut World) {
    if world.resource::<WindowGeometry>().maximized {
        world.trigger(crate::window_command::MaximizeWindow {
            maximized: Some(true),
        });
    }
}

/// Mirror the live primary `Window` back into [`WindowGeometry`] when it
/// moves or resizes. `lunco-settings` persists the resource on change;
/// the equality guard here keeps unrelated `Window` mutations (cursor,
/// focus) from marking settings dirty.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn save_window_geometry(
    win: Query<&Window, (With<PrimaryWindow>, Changed<Window>)>,
    maximized: Res<WindowMaximized>,
    mut geom: ResMut<WindowGeometry>,
) {
    let Ok(w) = win.single() else { return };
    let (x, y) = match w.position {
        WindowPosition::At(p) => (Some(p.x), Some(p.y)),
        _ => (geom.x, geom.y),
    };
    let next = WindowGeometry {
        width: w.resolution.width(),
        height: w.resolution.height(),
        x,
        y,
        maximized: maximized.0,
    };
    if *geom != next {
        *geom = next;
    }
}
