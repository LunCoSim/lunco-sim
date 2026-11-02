//! Process-startup console preference. The native shell consumes it before
//! CLI dispatch or logging; the workbench edits the next launch's preference.

use bevy::prelude::*;
use lunco_core::{Command, MarkClientLocalExt, on_command, register_commands};
use lunco_settings::{AppSettingsExt, SettingsSection};
use serde::{Deserialize, Serialize};

#[cfg(windows)]
mod windows;

/// Windows console visibility on subsequent desktop launches.
#[derive(Resource, Default, Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
struct DebugConsoleSettings {
    show_on_startup: bool,
}

impl SettingsSection for DebugConsoleSettings {
    const KEY: &'static str = "debug_console";
}

/// Persist whether Windows desktop launches open a debug console.
/// Takes effect on the next start and remains enabled until switched off.
#[Command]
pub struct SetDebugConsoleStartup {
    pub enabled: bool,
}

#[on_command(SetDebugConsoleStartup)]
fn set_debug_console_startup(
    trigger: On<SetDebugConsoleStartup>,
    mut settings: ResMut<DebugConsoleSettings>,
) {
    if settings.show_on_startup != cmd.enabled {
        settings.show_on_startup = cmd.enabled;
    }
}

register_commands!(set_debug_console_startup);

pub(crate) fn install(app: &mut App) {
    app.register_settings_section::<DebugConsoleSettings>();
    register_all_commands(app);
    app.mark_client_local::<SetDebugConsoleStartup>();
    if cfg!(windows) {
        app.add_systems(Startup, register_debug_menu);
    }
}

fn register_debug_menu(mut menus: ResMut<lunco_workbench_core::WorkbenchMenuRegistry>) {
    menus.register_settings_submenu("Debug", |ui, ctx| {
        let Some(settings) = ctx.resource::<DebugConsoleSettings>() else {
            return;
        };
        let mut enabled = settings.show_on_startup;
        if ui
            .checkbox(&mut enabled, "Show console on next start")
            .on_hover_text(
                "Show startup logs in a Windows console. Restart to apply; \
                 the console stays enabled until you turn this off.",
            )
            .changed()
        {
            ctx.trigger(SetDebugConsoleStartup { enabled });
        }
    });
}

/// Set up Windows standard streams before any CLI output or Bevy logging.
///
/// This is a process boundary: Rhai and the workbench do not exist yet.
/// Terminal launches attach to their parent's console; desktop launches only
/// allocate one when explicitly requested by the preference or `--console`.
#[cfg(windows)]
pub fn initialize_windows_console() {
    let requested = std::env::args_os().any(|arg| arg == "--console")
        || lunco_settings::load_section_from_disk::<DebugConsoleSettings>().show_on_startup;
    if let Err(error) = windows::initialize(requested) {
        windows::report_startup_error(&error);
        std::process::exit(1);
    }
}
