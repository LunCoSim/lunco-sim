//! OS window commands — Minimize / Maximize / Close — and the
//! `merged_titlebar_window()` helper that configures a `Window` so the
//! egui menu bar in [`crate::WorkbenchPlugin`] doubles as the OS title
//! bar (decorations off on Linux/Windows, transparent fullsize titlebar on
//! macOS).
//!
//! Buttons in the merged title bar fire these commands rather than
//! mutating the `Window` component directly, so anything that drives
//! the API (HTTP, scripts, MCP, hotkeys) gets the same behaviour.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use lunco_core::{on_command, register_commands, Command};

/// Mirrors the last-requested maximize state. Bevy's `Window` exposes
/// `set_maximized(bool)` but no symmetric reader, so we keep the bit
/// here for the toggle path and for the title-bar's button label.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct WindowMaximized(pub bool);

/// Minimize the primary OS window.
#[Command(default)]
pub struct MinimizeWindow {}

/// Maximize / restore the primary OS window. `maximized = None`
/// toggles based on [`WindowMaximized`].
#[Command(default)]
pub struct MaximizeWindow {
    pub maximized: Option<bool>,
}

/// Close the primary window (sends `AppExit::Success`).
#[Command(default)]
pub struct CloseWindow {}

/// Whether the native maximize request needs a monitor-size fallback.
///
/// Windows and macOS perform `Window::set_maximized` themselves. A second,
/// manual `WindowResolution` update races that native resize and can make DX12
/// reconfigure its swapchain while a backbuffer is still in use. A few Linux
/// tiling compositors ignore the native request, so only they need the fallback.
fn needs_manual_maximize_fallback() -> bool {
    cfg!(target_os = "linux")
}

#[on_command(MinimizeWindow)]
fn on_minimize(_trigger: On<MinimizeWindow>, mut q: Query<&mut Window, With<PrimaryWindow>>) {
    if let Ok(mut w) = q.single_mut() {
        w.set_minimized(true);
    }
}

#[on_command(MaximizeWindow)]
fn on_maximize(
    trigger: On<MaximizeWindow>,
    mut q: Query<&mut Window, With<PrimaryWindow>>,
    mut state: ResMut<WindowMaximized>,
    primary_monitor: Query<&bevy::window::Monitor, With<bevy::window::PrimaryMonitor>>,
    any_monitor: Query<&bevy::window::Monitor>,
) {
    let target = trigger.event().maximized.unwrap_or(!state.0);
    state.0 = target;
    let Ok(mut window) = q.single_mut() else {
        return;
    };
    window.set_maximized(target);
    // Some Linux compositors (sway/i3, several tilers) ignore the native
    // request, so only they resize to the primary monitor as a fallback. On
    // restore we leave winit to size things back.
    if target && needs_manual_maximize_fallback() {
        let monitor = primary_monitor
            .single()
            .ok()
            .or_else(|| any_monitor.iter().next());
        if let Some(monitor) = monitor {
            let scale = monitor.scale_factor as f32;
            window.resolution.set(
                monitor.physical_width as f32 / scale,
                monitor.physical_height as f32 / scale,
            );
            window.position = bevy::window::WindowPosition::At(bevy::math::IVec2::ZERO);
        }
    }
}

#[on_command(CloseWindow)]
fn on_close(
    _trigger: On<CloseWindow>,
    mut messages: ResMut<bevy::ecs::message::Messages<bevy::app::AppExit>>,
) {
    messages.write(bevy::app::AppExit::Success);
}

register_commands!(on_minimize, on_maximize, on_close,);

#[cfg(test)]
mod tests {
    use super::needs_manual_maximize_fallback;

    #[test]
    fn only_linux_uses_the_manual_maximize_fallback() {
        assert_eq!(needs_manual_maximize_fallback(), cfg!(target_os = "linux"));
    }
}

/// Plugin registering the window-control commands ([`MinimizeWindow`],
/// [`MaximizeWindow`], [`CloseWindow`]) and the [`WindowMaximized`] resource.
pub struct WindowCommandPlugin;

impl Plugin for WindowCommandPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WindowMaximized>();
        register_all_commands(app);
    }
}

/// Configure a `Window` so the workbench's egui menu bar replaces the OS title
/// bar on Linux/Windows; macOS uses a transparent fullsize titlebar. Spread it
/// into your `WindowPlugin`:
///
/// ```ignore
/// WindowPlugin {
///     primary_window: Some(lunco_workbench::merged_titlebar_window("My App")),
///     ..default()
/// }
/// ```
pub fn merged_titlebar_window(title: impl Into<String>) -> Window {
    Window {
        title: title.into(),
        #[cfg(not(target_os = "macos"))]
        decorations: false,
        #[cfg(target_os = "macos")]
        titlebar_transparent: true,
        #[cfg(target_os = "macos")]
        titlebar_show_title: false,
        #[cfg(target_os = "macos")]
        fullsize_content_view: true,
        ..default()
    }
}
