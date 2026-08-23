//! Native Velopack update checks for the interactive desktop application.
//!
//! The updater is deliberately a UI concern.  The package itself is the
//! complete staged application directory, so Velopack applies the new package
//! as one unit: assets added by a release arrive, and files absent from the
//! release are not retained as an accidental second asset tree.

use std::sync::{mpsc, Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use bevy::prelude::*;
use bevy::tasks::{futures_lite::future, IoTaskPool, Task};
use bevy_egui::egui;
use lunco_settings::AppSettingsExt;
use lunco_workbench::status_bus::{StatusBarAction, StatusBus, StatusLevel};
use lunco_workbench::WorkbenchLayout;
use serde::{Deserialize, Serialize};
use velopack::sources::GithubSource;
use velopack::{UpdateCheck, UpdateInfo, UpdateManager};

const UPDATE_STATUS_SOURCE: &str = "updates";
/// Public machine-only repository containing immutable update releases.
pub(crate) const UPDATE_REPOSITORY: &str = "https://github.com/LunCoSim/lunco-sim-updates";
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const UPDATE_CHANNEL: &str = "win-x64";
#[cfg(all(target_os = "windows", target_arch = "aarch64"))]
const UPDATE_CHANNEL: &str = "win-arm64";
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const UPDATE_CHANNEL: &str = "osx-x64";
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const UPDATE_CHANNEL: &str = "osx-arm64";
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const UPDATE_CHANNEL: &str = "linux-x64";
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const UPDATE_CHANNEL: &str = "linux-arm64";

#[cfg(target_os = "linux")]
const UPDATE_PACKAGE_GUIDANCE: &str =
    "On Linux, run the official Velopack .AppImage from a writable location and keep launching that same file; the update replaces it in place.";
#[cfg(target_os = "windows")]
const UPDATE_PACKAGE_GUIDANCE: &str =
    "On Windows, install the official Setup.exe and launch the installed LunCoSim shortcut; updates replace the installed application and restart it.";
#[cfg(target_os = "macos")]
const UPDATE_PACKAGE_GUIDANCE: &str =
    "On macOS, install the official .pkg for your CPU and launch the installed LunCoSim.app; updates replace the app bundle and restart it.";
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
const UPDATE_PACKAGE_GUIDANCE: &str =
    "Run the official Velopack package from its installed location; source builds and ordinary archives are not update-managed.";

/// Persisted preference for the native desktop updater.
#[derive(Resource, Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub(crate) struct UpdateSettings {
    /// Check once when the GUI starts.
    pub auto_check: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self { auto_check: true }
    }
}

impl lunco_settings::SettingsSection for UpdateSettings {
    const KEY: &'static str = "updates";
}

/// User-visible state of the update operation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum UpdateStatus {
    #[default]
    Idle,
    Checking,
    Available,
    Downloading,
    ReadyToRestart,
    NotInstalled,
    Error,
}

/// UI view-model.  The Velopack `UpdateInfo` is retained between the check,
/// download, and apply actions so the exact package selected by the feed is
/// the one that gets installed.
#[derive(Resource, Clone)]
pub(crate) struct UpdateState {
    pub(crate) status: UpdateStatus,
    pub(crate) available: Option<UpdateInfo>,
    pub(crate) ready: Option<UpdateInfo>,
    pub(crate) error: Option<String>,
    /// Latest percentage reported by Velopack while downloading.
    pub(crate) download_progress: Option<u8>,
    /// Unix time when the current process last attempted a check, including a
    /// manual check. It also prevents the startup check from being scheduled
    /// again in every subsequent frame.
    pub(crate) last_check_unix: Option<u64>,
}

impl Default for UpdateState {
    fn default() -> Self {
        Self {
            status: UpdateStatus::Idle,
            available: None,
            ready: None,
            error: None,
            download_progress: None,
            last_check_unix: None,
        }
    }
}

/// Typed intents emitted by the Settings menu.  The menu never starts
/// network or filesystem work directly.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct UpdateActions {
    pub(crate) check_requested: bool,
    pub(crate) download_requested: bool,
    pub(crate) apply_requested: bool,
}

#[derive(Resource, Default)]
struct UpdateTasks {
    check: Option<Task<UpdateCheckResult>>,
    download: Option<Task<UpdateDownloadResult>>,
    download_progress: Option<Arc<Mutex<mpsc::Receiver<i16>>>>,
}

enum UpdateCheckResult {
    Available(Box<UpdateInfo>),
    NoUpdate,
    NotInstalled,
    Error(String),
}

struct UpdateDownloadResult {
    info: UpdateInfo,
    result: Result<(), String>,
}

#[derive(Default)]
struct UpdateStatusMemo {
    status: Option<UpdateStatus>,
    detail: String,
}

/// Whether the short update window is currently visible. The status bar remains
/// the durable progress surface; this window is the actionable hand-off for a
/// discovered or downloaded update and can be dismissed without cancelling work.
#[derive(Resource, Clone, Copy, Debug, Default)]
struct UpdateDialogState {
    open: bool,
    last_status: Option<UpdateStatus>,
}

pub(crate) struct UpdatePlugin;

impl Plugin for UpdatePlugin {
    fn build(&self, app: &mut App) {
        app.register_settings_section::<UpdateSettings>()
            .init_resource::<UpdateState>()
            .init_resource::<UpdateActions>()
            .init_resource::<UpdateTasks>()
            .init_resource::<UpdateDialogState>()
            .add_systems(Startup, register_update_settings_menu)
            .add_systems(
                Update,
                (
                    schedule_automatic_check,
                    process_update_actions,
                    poll_update_tasks,
                    open_update_dialog_on_transition,
                    mirror_update_status_to_status_bus,
                )
                    .chain(),
            )
            .add_observer(on_status_bar_action)
            .add_systems(
                bevy_egui::EguiPrimaryContextPass,
                render_update_dialog.in_set(lunco_workbench::ApplicationOverlayRenderSet),
            );
    }
}

/// Continue the update flow from the shared status bar. The status bar only
/// emits the source key; this updater owns the phase-specific meaning.
fn on_status_bar_action(
    trigger: On<StatusBarAction>,
    state: Res<UpdateState>,
    mut actions: ResMut<UpdateActions>,
) {
    queue_status_bar_action(trigger.event().source, &state, &mut actions);
}

fn queue_status_bar_action(source: &str, state: &UpdateState, actions: &mut UpdateActions) {
    if source != UPDATE_STATUS_SOURCE {
        return;
    }

    match state.status {
        UpdateStatus::Available => actions.download_requested = true,
        UpdateStatus::ReadyToRestart => actions.apply_requested = true,
        UpdateStatus::Error if state.ready.is_some() => actions.apply_requested = true,
        UpdateStatus::Checking
        | UpdateStatus::Downloading
        | UpdateStatus::Idle
        | UpdateStatus::NotInstalled
        | UpdateStatus::Error => {}
    }
}

fn register_update_settings_menu(world: &mut World) {
    let Some(mut layout) = world.get_resource_mut::<WorkbenchLayout>() else {
        return;
    };
    layout.register_settings_submenu("Updates", |ui, ctx| {
        // Keep the whole updater flow readable when the menu is opened on a
        // narrow window. The persistent dialog below uses the same width budget.
        ui.set_min_width(560.0);
        ui.label(egui::RichText::new("Velopack updates").weak().small());
        if let Some(identity) = ctx.resource::<lunco_workbench::BuildIdentity>() {
            ui.label(identity.version_label());
        }
        ui.label(format!(
            "LunCoSim nightly updates · {} channel",
            UPDATE_CHANNEL
        ));
        egui::CollapsingHeader::new("How updates work")
            .default_open(false)
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(
                        "LunCoSim checks once at startup. When it finds a release, click Download update in the status bar or here. Nothing restarts until you approve installation.",
                    )
                    .wrap(),
                );
                ui.add(egui::Label::new(UPDATE_PACKAGE_GUIDANCE).wrap());
                ui.add(
                    egui::Label::new(
                        "Source builds, target/debug binaries, and ordinary archives are not update-managed.",
                    )
                    .wrap(),
                );
            });
        ui.add_space(6.0);

        let Some(mut settings) = ctx.resource::<UpdateSettings>().cloned() else {
            return;
        };
        let original_settings = settings.clone();
        ui.checkbox(&mut settings.auto_check, "Check for updates at startup");
        if settings != original_settings {
            ctx.set_resource(settings.clone());
        }

        let Some(state) = ctx.resource::<UpdateState>().cloned() else {
            return;
        };
        if let Some(last_check_unix) = state.last_check_unix {
            ui.label(format_last_check(last_check_unix));
        } else {
            ui.label("No update check has run in this process.");
        }
        match state.status {
            UpdateStatus::Idle => {
                ui.label("No update is available.");
            }
            UpdateStatus::Checking => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Checking GitHub for a newer release…");
                });
            }
            UpdateStatus::Available => {
                if let Some(info) = state.available.as_ref() {
                    ui.label(format!(
                        "Update available: {}",
                        info.TargetFullRelease.Version
                    ));
                    if state.error.is_some() {
                        ui.label("The download failed. Retry it below.");
                    } else {
                        ui.label("Click Download update to start.");
                    }
                }
            }
            UpdateStatus::Downloading => {
                let progress = state.download_progress.unwrap_or_default();
                ui.label(format!("Downloading update… {progress}%"));
                ui.add_sized(
                    [ui.available_width(), 24.0],
                    egui::ProgressBar::new(f32::from(progress) / 100.0).show_percentage(),
                );
                ui.label("You can keep working while the download completes.");
            }
            UpdateStatus::ReadyToRestart => {
                ui.label("Update downloaded and ready to install.");
            }
            UpdateStatus::NotInstalled => {
                ui.label("This build is not running from an update-managed Velopack package.");
                ui.label(UPDATE_PACKAGE_GUIDANCE);
            }
            UpdateStatus::Error => {
                if let Some(error) = state.error.as_deref() {
                    if let Some(theme) = ctx.resource::<lunco_theme::Theme>() {
                        ui.label(egui::RichText::new(error).color(theme.tokens.error));
                    } else {
                        ui.label(error);
                    }
                }
                if state.ready.is_some() {
                    ui.label("The downloaded update is still ready to install.");
                }
            }
        }

        let mut actions = ctx.resource::<UpdateActions>().copied().unwrap_or_default();
        let original_actions = actions;
        let can_check = matches!(
            state.status,
            UpdateStatus::Idle | UpdateStatus::NotInstalled | UpdateStatus::Error
        ) && state.ready.is_none();
        ui.horizontal(|ui| {
            if can_check && ui.button("Check now").clicked() {
                actions.check_requested = true;
            }
            let download_label = if state.error.is_some() {
                "Retry download"
            } else {
                "Download update"
            };
            if state.status == UpdateStatus::Available && ui.button(download_label).clicked() {
                actions.download_requested = true;
            }
            if state.ready.is_some() && ui.button("Restart to install").clicked() {
                actions.apply_requested = true;
            }
        });
        if actions != original_actions {
            ctx.set_resource(actions);
        }
    });
}

fn schedule_automatic_check(
    settings: Option<Res<UpdateSettings>>,
    state: Res<UpdateState>,
    mut actions: ResMut<UpdateActions>,
) {
    let Some(settings) = settings else {
        return;
    };
    if !automatic_check_due(&settings, &state, &actions) {
        return;
    }
    actions.check_requested = true;
}

fn automatic_check_due(
    settings: &UpdateSettings,
    state: &UpdateState,
    actions: &UpdateActions,
) -> bool {
    settings.auto_check
        && state.status == UpdateStatus::Idle
        && !actions.check_requested
        // A process performs one startup check. Manual checks remain available
        // through the menu after that check completes.
        && state.last_check_unix.is_none()
}

fn process_update_actions(
    mut actions: ResMut<UpdateActions>,
    mut state: ResMut<UpdateState>,
    mut tasks: ResMut<UpdateTasks>,
) {
    if actions.check_requested {
        actions.check_requested = false;
        let can_start_check = matches!(
            state.status,
            UpdateStatus::Idle
                | UpdateStatus::Available
                | UpdateStatus::NotInstalled
                | UpdateStatus::Error
        ) && state.ready.is_none();
        if can_start_check && tasks.check.is_none() && tasks.download.is_none() {
            let now = unix_now();
            state.last_check_unix = Some(now);
            state.status = UpdateStatus::Checking;
            state.available = None;
            state.ready = None;
            state.error = None;
            state.download_progress = None;
            tasks.check = Some(IoTaskPool::get().spawn(async { check_for_updates() }));
        }
    }

    if actions.download_requested {
        actions.download_requested = false;
        if tasks.check.is_none() && tasks.download.is_none() {
            if let Some(info) = state.available.clone() {
                let (progress_sender, progress_receiver) = mpsc::channel();
                state.status = UpdateStatus::Downloading;
                state.error = None;
                state.download_progress = Some(0);
                tasks.download_progress = Some(Arc::new(Mutex::new(progress_receiver)));
                tasks.download = Some(
                    IoTaskPool::get().spawn(async move { download_update(info, progress_sender) }),
                );
            }
        }
    }

    if actions.apply_requested {
        actions.apply_requested = false;
        let Some(info) = state.ready.clone() else {
            return;
        };
        match create_update_manager() {
            Ok(manager) => {
                if let Err(error) = manager.apply_updates_and_restart(&info) {
                    state.status = UpdateStatus::Error;
                    state.error = Some(format!("Could not install update: {error}"));
                }
            }
            Err(error) => {
                state.status = UpdateStatus::Error;
                state.error = Some(format!("Could not prepare update: {error}"));
            }
        }
    }
}

fn poll_update_tasks(mut state: ResMut<UpdateState>, mut tasks: ResMut<UpdateTasks>) {
    if let Some(progress_receiver) = tasks.download_progress.as_ref() {
        if let Ok(progress_receiver) = progress_receiver.lock() {
            for progress in progress_receiver.try_iter() {
                state.download_progress = Some(clamp_download_progress(progress));
            }
        }
    }

    let check_result = tasks
        .check
        .as_mut()
        .and_then(|task| future::block_on(future::poll_once(task)));
    if let Some(result) = check_result {
        tasks.check = None;
        match result {
            UpdateCheckResult::Available(info) => {
                state.status = UpdateStatus::Available;
                state.available = Some(*info);
                state.download_progress = None;
            }
            UpdateCheckResult::NoUpdate => {
                state.status = UpdateStatus::Idle;
                state.error = None;
                state.download_progress = None;
            }
            UpdateCheckResult::NotInstalled => {
                state.status = UpdateStatus::NotInstalled;
                state.error = None;
                state.download_progress = None;
            }
            UpdateCheckResult::Error(error) => {
                state.status = UpdateStatus::Error;
                state.error = Some(error);
                state.download_progress = None;
            }
        }
    }

    let download_result = tasks
        .download
        .as_mut()
        .and_then(|task| future::block_on(future::poll_once(task)));
    if let Some(result) = download_result {
        tasks.download = None;
        tasks.download_progress = None;
        match result.result {
            Ok(()) => {
                state.status = UpdateStatus::ReadyToRestart;
                state.ready = Some(result.info);
                state.error = None;
                state.download_progress = Some(100);
            }
            Err(error) => {
                state.status = UpdateStatus::Available;
                state.available = Some(result.info);
                state.error = Some(error);
                state.download_progress = None;
            }
        }
    }
}

fn open_update_dialog_on_transition(
    state: Res<UpdateState>,
    mut dialog: ResMut<UpdateDialogState>,
) {
    if dialog.last_status != Some(state.status) {
        if matches!(
            state.status,
            UpdateStatus::Available | UpdateStatus::Downloading | UpdateStatus::ReadyToRestart
        ) {
            dialog.open = true;
        }
        dialog.last_status = Some(state.status);
    }
}

fn render_update_dialog(
    mut egui_ctx: bevy_egui::EguiContexts,
    state: Res<UpdateState>,
    theme: Res<lunco_theme::Theme>,
    mut actions: ResMut<UpdateActions>,
    mut dialog: ResMut<UpdateDialogState>,
) {
    if !dialog.open {
        return;
    }
    let Ok(ctx) = egui_ctx.ctx_mut() else {
        return;
    };

    let mut open = true;
    let mut close_requested = false;
    egui::Window::new("LunCoSim update")
        .id(egui::Id::new("lunco_update_dialog"))
        .open(&mut open)
        .order(egui::Order::Foreground)
        .collapsible(false)
        .resizable(true)
        .default_width(560.0)
        .min_width(520.0)
        .max_width(760.0)
        .show(ctx, |ui| {
            ui.heading("LunCoSim update");
            ui.add_space(4.0);

            match state.status {
                UpdateStatus::Available => {
                    let version = state
                        .available
                        .as_ref()
                        .map(|info| info.TargetFullRelease.Version.as_str())
                        .unwrap_or("new release");
                    ui.label(format!("Version {version} is available."));
                    if let Some(error) = state.error.as_deref() {
                        ui.colored_label(theme.tokens.error, error);
                        if ui.button("Retry download").clicked() {
                            actions.download_requested = true;
                        }
                    } else {
                        ui.label("The update is ready to download.");
                        if ui.button("Download update").clicked() {
                            actions.download_requested = true;
                        }
                    }
                }
                UpdateStatus::Downloading => {
                    let progress = state.download_progress.unwrap_or_default();
                    ui.label("Downloading. You can keep working.");
                    ui.add_sized(
                        [ui.available_width(), 24.0],
                        egui::ProgressBar::new(f32::from(progress) / 100.0).show_percentage(),
                    );
                }
                UpdateStatus::ReadyToRestart => {
                    ui.label("The update is downloaded and ready to install.");
                    ui.label(
                        "Save your work before restarting; LunCoSim will close and reopen once.",
                    );
                    if ui
                        .add_sized(
                            [ui.available_width(), 32.0],
                            egui::Button::new("Restart to install update"),
                        )
                        .clicked()
                    {
                        actions.apply_requested = true;
                    }
                }
                UpdateStatus::Error if state.ready.is_some() => {
                    if let Some(error) = state.error.as_deref() {
                        ui.colored_label(theme.tokens.error, error);
                    }
                    ui.label("The downloaded update is still ready to install.");
                    if ui
                        .add_sized(
                            [ui.available_width(), 32.0],
                            egui::Button::new("Retry install and restart"),
                        )
                        .clicked()
                    {
                        actions.apply_requested = true;
                    }
                }
                _ => {}
            }

            ui.add_space(8.0);
            if ui.button("Later").clicked() {
                close_requested = true;
            }
        });
    dialog.open = open && !close_requested;
}

/// Mirror updater state into the shared status bar without owning or polling
/// the network task itself. The updater task publishes progress through
/// [`UpdateState`]; this state mirror only performs cheap, non-blocking UI
/// projection on the main thread.
fn mirror_update_status_to_status_bus(
    state: Res<UpdateState>,
    bus: Option<ResMut<StatusBus>>,
    mut last: Local<UpdateStatusMemo>,
) {
    let Some(mut bus) = bus else {
        return;
    };

    let detail = update_status_detail(&state);
    let changed = last.status != Some(state.status) || last.detail != detail;
    match state.status {
        UpdateStatus::Checking => {
            bus.set_progress(
                UPDATE_STATUS_SOURCE,
                "checking GitHub for a newer release…",
                0,
                0,
            );
        }
        UpdateStatus::Downloading => {
            let progress = u64::from(state.download_progress.unwrap_or_default());
            let version = state
                .available
                .as_ref()
                .map(|info| info.TargetFullRelease.Version.as_str())
                .unwrap_or("update");
            bus.set_progress(
                UPDATE_STATUS_SOURCE,
                format!("downloading {version}… {progress}%"),
                progress,
                100,
            );
        }
        UpdateStatus::Available => {
            bus.remove_progress(UPDATE_STATUS_SOURCE);
            if changed {
                let version = state
                    .available
                    .as_ref()
                    .map(|info| info.TargetFullRelease.Version.as_str())
                    .unwrap_or("a newer release");
                if let Some(error) = state.error.as_deref() {
                    bus.push(
                        UPDATE_STATUS_SOURCE,
                        StatusLevel::Attention,
                        format!("Update download failed: {error} · Retry download"),
                    );
                } else {
                    bus.push(
                        UPDATE_STATUS_SOURCE,
                        StatusLevel::Attention,
                        format!("Update available: {version} · Download update"),
                    );
                }
            }
        }
        UpdateStatus::ReadyToRestart => {
            bus.remove_progress(UPDATE_STATUS_SOURCE);
            if changed {
                let version = state
                    .ready
                    .as_ref()
                    .map(|info| info.TargetFullRelease.Version.as_str())
                    .unwrap_or("update");
                bus.push(
                    UPDATE_STATUS_SOURCE,
                    StatusLevel::Attention,
                    format!("Update {version} ready · Install and restart"),
                );
            }
        }
        UpdateStatus::Idle => {
            bus.remove_progress(UPDATE_STATUS_SOURCE);
            if changed && state.last_check_unix.is_some() {
                bus.push(
                    UPDATE_STATUS_SOURCE,
                    StatusLevel::Info,
                    "no update available",
                );
            }
        }
        UpdateStatus::NotInstalled => {
            bus.remove_progress(UPDATE_STATUS_SOURCE);
            if changed {
                bus.push(
                    UPDATE_STATUS_SOURCE,
                    StatusLevel::Warn,
                    "updates unavailable: this build is not Velopack-managed",
                );
            }
        }
        UpdateStatus::Error => {
            bus.remove_progress(UPDATE_STATUS_SOURCE);
            if changed {
                let error = state.error.as_deref().unwrap_or("unknown error");
                if state.ready.is_some() {
                    bus.push(
                        UPDATE_STATUS_SOURCE,
                        StatusLevel::Attention,
                        format!("Update install failed: {error} · Retry install"),
                    );
                } else {
                    bus.push(
                        UPDATE_STATUS_SOURCE,
                        StatusLevel::Error,
                        format!("update check failed: {error}"),
                    );
                }
            }
        }
    }

    last.status = Some(state.status);
    last.detail = detail;
}

fn update_status_detail(state: &UpdateState) -> String {
    match state.status {
        UpdateStatus::Available => format!(
            "{}:{}",
            state
                .available
                .as_ref()
                .map(|info| info.TargetFullRelease.Version.as_str())
                .unwrap_or(""),
            state.error.as_deref().unwrap_or("")
        ),
        UpdateStatus::ReadyToRestart => state
            .ready
            .as_ref()
            .map(|info| info.TargetFullRelease.Version.clone())
            .unwrap_or_default(),
        UpdateStatus::Error => state.error.clone().unwrap_or_default(),
        _ => String::new(),
    }
}

fn check_for_updates() -> UpdateCheckResult {
    let manager = match create_update_manager() {
        Ok(manager) => manager,
        Err(velopack::Error::NotInstalled(_)) => return UpdateCheckResult::NotInstalled,
        Err(error) => return UpdateCheckResult::Error(error.to_string()),
    };
    match manager.check_for_updates() {
        Ok(UpdateCheck::UpdateAvailable(info)) => UpdateCheckResult::Available(info),
        Ok(UpdateCheck::NoUpdateAvailable | UpdateCheck::RemoteIsEmpty) => {
            UpdateCheckResult::NoUpdate
        }
        Err(error) => UpdateCheckResult::Error(error.to_string()),
    }
}

fn download_update(info: UpdateInfo, progress: mpsc::Sender<i16>) -> UpdateDownloadResult {
    let result = create_update_manager()
        .and_then(|manager| manager.download_updates(&info, Some(progress)))
        .map_err(|error| error.to_string());
    UpdateDownloadResult { info, result }
}

fn clamp_download_progress(progress: i16) -> u8 {
    progress.clamp(0, 100) as u8
}

fn create_update_manager() -> Result<UpdateManager, velopack::Error> {
    let source = GithubSource::new(UPDATE_REPOSITORY, None, true);
    UpdateManager::new(
        source,
        Some(velopack::UpdateOptions {
            ExplicitChannel: Some(UPDATE_CHANNEL.to_string()),
            ..Default::default()
        }),
        None,
    )
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn format_last_check(timestamp: u64) -> String {
    let elapsed = unix_now().saturating_sub(timestamp);
    let age = match elapsed {
        0..=4 => "just now".to_string(),
        5..=59 => format!("{elapsed} seconds ago"),
        60..=3599 => format!("{} minutes ago", elapsed / 60),
        3600..=86_399 => format!("{} hours ago", elapsed / 3600),
        _ => format!("{} days ago", elapsed / 86_400),
    };
    format!("Last checked: {age}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_check_runs_once_per_process() {
        let settings = UpdateSettings::default();
        let state = UpdateState::default();
        let actions = UpdateActions::default();

        assert!(automatic_check_due(&settings, &state, &actions));

        let mut state = state;
        state.last_check_unix = Some(100);
        assert!(!automatic_check_due(&settings, &state, &actions));
    }

    #[test]
    fn automatic_check_does_not_duplicate_pending_or_disabled_checks() {
        let mut settings = UpdateSettings::default();
        let state = UpdateState::default();
        let actions = UpdateActions {
            check_requested: true,
            ..Default::default()
        };
        assert!(!automatic_check_due(&settings, &state, &actions));

        let actions = UpdateActions::default();
        settings.auto_check = false;
        assert!(!automatic_check_due(&settings, &state, &actions));
    }

    #[test]
    fn discovered_updates_require_an_explicit_download_action() {
        let settings = UpdateSettings::default();
        assert!(settings.auto_check);
    }

    #[test]
    fn download_progress_is_clamped_to_percentage_range() {
        assert_eq!(clamp_download_progress(-1), 0);
        assert_eq!(clamp_download_progress(42), 42);
        assert_eq!(clamp_download_progress(101), 100);
    }

    #[test]
    fn status_bar_click_starts_download_for_available_update() {
        let state = UpdateState {
            status: UpdateStatus::Available,
            ..Default::default()
        };
        let mut actions = UpdateActions::default();

        queue_status_bar_action(UPDATE_STATUS_SOURCE, &state, &mut actions);

        assert!(actions.download_requested);
        assert!(!actions.apply_requested);
    }

    #[test]
    fn status_bar_click_installs_downloaded_update() {
        let state = UpdateState {
            status: UpdateStatus::ReadyToRestart,
            ..Default::default()
        };
        let mut actions = UpdateActions::default();

        queue_status_bar_action(UPDATE_STATUS_SOURCE, &state, &mut actions);

        assert!(!actions.download_requested);
        assert!(actions.apply_requested);
    }

    #[test]
    fn status_bar_click_ignores_other_sources_and_in_flight_downloads() {
        let state = UpdateState {
            status: UpdateStatus::Downloading,
            ..Default::default()
        };
        let mut actions = UpdateActions::default();

        queue_status_bar_action("scene", &state, &mut actions);
        queue_status_bar_action(UPDATE_STATUS_SOURCE, &state, &mut actions);

        assert_eq!(actions, UpdateActions::default());
    }
}
