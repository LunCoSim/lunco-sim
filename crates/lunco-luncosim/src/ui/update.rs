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
use lunco_workbench::status_bus::{StatusBus, StatusLevel};
use lunco_workbench::WorkbenchLayout;
use serde::{Deserialize, Serialize};
use velopack::sources::GithubSource;
use velopack::{UpdateCheck, UpdateInfo, UpdateManager};

const UPDATE_CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;
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
    /// Check once per day when the GUI starts.
    pub auto_check: bool,
    /// Unix time of the most recent automatic check.
    pub last_check_unix: u64,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            auto_check: true,
            last_check_unix: 0,
        }
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
    /// manual check. This is separate from the persisted automatic-check
    /// throttle so the UI remains truthful after a successful no-update result.
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

#[derive(Resource)]
struct UpdateTasks {
    check: Option<Task<UpdateCheckResult>>,
    download: Option<Task<UpdateDownloadResult>>,
    download_progress: Option<Arc<Mutex<mpsc::Receiver<i16>>>>,
}

impl Default for UpdateTasks {
    fn default() -> Self {
        Self {
            check: None,
            download: None,
            download_progress: None,
        }
    }
}

enum UpdateCheckResult {
    Available(UpdateInfo),
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

pub(crate) struct UpdatePlugin;

impl Plugin for UpdatePlugin {
    fn build(&self, app: &mut App) {
        app.register_settings_section::<UpdateSettings>()
            .init_resource::<UpdateState>()
            .init_resource::<UpdateActions>()
            .init_resource::<UpdateTasks>()
            .add_systems(Startup, register_update_settings_menu)
            .add_systems(
                Update,
                (
                    schedule_automatic_check,
                    process_update_actions,
                    poll_update_tasks,
                    mirror_update_status_to_status_bus,
                )
                    .chain(),
            );
    }
}

fn register_update_settings_menu(world: &mut World) {
    let Some(mut layout) = world.get_resource_mut::<WorkbenchLayout>() else {
        return;
    };
    layout.register_settings_submenu("Updates", |ui, ctx| {
        ui.label(egui::RichText::new("Velopack updates").weak().small());
        ui.label(format!(
            "Version {} ({})",
            crate::PRODUCT_VERSION,
            crate::GIT_SHA
        ));
        ui.label(format!(
            "LunCoSim nightly updates · {} channel",
            UPDATE_CHANNEL
        ));
        egui::CollapsingHeader::new("How automatic updates work")
            .default_open(false)
            .show(ui, |ui| {
                ui.label("LunCoSim checks at most once per 24 hours when this GUI starts.");
                ui.label(
                    "A check never installs anything by itself: choose Download update, then Install and restart.",
                );
                ui.label(
                    UPDATE_PACKAGE_GUIDANCE,
                );
                ui.label(
                    "Source builds, target/debug binaries, and ordinary archives are not update-managed.",
                );
            });
        ui.add_space(6.0);

        let Some(mut settings) = ctx.resource::<UpdateSettings>().cloned() else {
            return;
        };
        let original_settings = settings.clone();
        ui.checkbox(
            &mut settings.auto_check,
            "Check for updates at startup (at most once per day)",
        );
        if settings != original_settings {
            ctx.set_resource(settings);
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
                    ui.label("Choose Download update, then install and restart.");
                }
            }
            UpdateStatus::Downloading => {
                let progress = state.download_progress.unwrap_or_default();
                ui.label(format!("Downloading update… {progress}%"));
                ui.add(egui::ProgressBar::new(f32::from(progress) / 100.0).show_percentage());
            }
            UpdateStatus::ReadyToRestart => {
                ui.label("Update downloaded. Choose Install and restart.");
            }
            UpdateStatus::NotInstalled => {
                ui.label("This build is not running from an update-managed Velopack package.");
                ui.label(UPDATE_PACKAGE_GUIDANCE);
            }
            UpdateStatus::Error => {
                if let Some(error) = state.error.as_deref() {
                    ui.label(egui::RichText::new(error).color(egui::Color32::RED));
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
            if state.status == UpdateStatus::Available && ui.button("Download update").clicked() {
                actions.download_requested = true;
            }
            if state.ready.is_some() && ui.button("Install and restart").clicked() {
                actions.apply_requested = true;
            }
        });
        if actions != original_actions {
            ctx.set_resource(actions);
        }
    });
}

fn schedule_automatic_check(
    settings: Option<ResMut<UpdateSettings>>,
    state: Res<UpdateState>,
    mut actions: ResMut<UpdateActions>,
) {
    let Some(mut settings) = settings else {
        return;
    };
    let now = unix_now();
    if !automatic_check_due(&settings, &state, &actions, now) {
        return;
    }
    settings.last_check_unix = now;
    actions.check_requested = true;
}

fn automatic_check_due(
    settings: &UpdateSettings,
    state: &UpdateState,
    actions: &UpdateActions,
    now: u64,
) -> bool {
    settings.auto_check
        && state.status == UpdateStatus::Idle
        && !actions.check_requested
        && now.saturating_sub(settings.last_check_unix) >= UPDATE_CHECK_INTERVAL_SECS
}

fn process_update_actions(
    mut actions: ResMut<UpdateActions>,
    mut state: ResMut<UpdateState>,
    mut tasks: ResMut<UpdateTasks>,
    settings: Option<ResMut<UpdateSettings>>,
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
            if let Some(mut settings) = settings {
                // A manual check also satisfies the automatic-check throttle;
                // otherwise a manual check made after the 24-hour boundary
                // would immediately run a duplicate automatic check next frame.
                settings.last_check_unix = now;
            }
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
                state.available = Some(info);
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
                        StatusLevel::Error,
                        format!("update download failed: {error}"),
                    );
                } else {
                    bus.push(
                        UPDATE_STATUS_SOURCE,
                        StatusLevel::Info,
                        format!("update available: {version}"),
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
                    StatusLevel::Info,
                    format!("update {version} downloaded — restart to install"),
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
                bus.push(
                    UPDATE_STATUS_SOURCE,
                    StatusLevel::Error,
                    format!(
                        "update check failed: {}",
                        state.error.as_deref().unwrap_or("unknown error")
                    ),
                );
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
        Ok(UpdateCheck::UpdateAvailable(info)) => UpdateCheckResult::Available(*info),
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
    fn automatic_check_is_throttled() {
        let mut settings = UpdateSettings::default();
        settings.last_check_unix = 100;
        let state = UpdateState::default();
        let actions = UpdateActions::default();

        assert!(!automatic_check_due(
            &settings,
            &state,
            &actions,
            100 + UPDATE_CHECK_INTERVAL_SECS - 1
        ));
        assert!(automatic_check_due(
            &settings,
            &state,
            &actions,
            100 + UPDATE_CHECK_INTERVAL_SECS
        ));
    }

    #[test]
    fn automatic_check_does_not_duplicate_pending_or_disabled_checks() {
        let mut settings = UpdateSettings::default();
        let state = UpdateState::default();
        let mut actions = UpdateActions::default();

        actions.check_requested = true;
        assert!(!automatic_check_due(
            &settings,
            &state,
            &actions,
            UPDATE_CHECK_INTERVAL_SECS + 1
        ));

        actions.check_requested = false;
        settings.auto_check = false;
        assert!(!automatic_check_due(
            &settings,
            &state,
            &actions,
            UPDATE_CHECK_INTERVAL_SECS + 1
        ));
    }

    #[test]
    fn download_progress_is_clamped_to_percentage_range() {
        assert_eq!(clamp_download_progress(-1), 0);
        assert_eq!(clamp_download_progress(42), 42);
        assert_eq!(clamp_download_progress(101), 100);
    }
}
