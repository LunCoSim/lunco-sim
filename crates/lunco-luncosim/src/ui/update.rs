//! Native Velopack update checks for the interactive desktop application.
//!
//! The updater is deliberately a UI concern.  The package itself is the
//! complete staged application directory, so Velopack applies the new package
//! as one unit: assets added by a release arrive, and files absent from the
//! release are not retained as an accidental second asset tree.

use std::time::{SystemTime, UNIX_EPOCH};

use bevy::prelude::*;
use bevy::tasks::{futures_lite::future, IoTaskPool, Task};
use bevy_egui::egui;
use lunco_settings::AppSettingsExt;
use lunco_workbench::WorkbenchLayout;
use serde::{Deserialize, Serialize};
use velopack::sources::GithubSource;
use velopack::{UpdateCheck, UpdateInfo, UpdateManager};

const UPDATE_CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;
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
}

impl Default for UpdateState {
    fn default() -> Self {
        Self {
            status: UpdateStatus::Idle,
            available: None,
            ready: None,
            error: None,
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
        ui.add_space(6.0);

        let Some(mut settings) = ctx.resource::<UpdateSettings>().cloned() else {
            return;
        };
        let original_settings = settings.clone();
        ui.checkbox(&mut settings.auto_check, "Check once per day at startup");
        if settings != original_settings {
            ctx.set_resource(settings);
        }

        let Some(state) = ctx.resource::<UpdateState>().cloned() else {
            return;
        };
        match state.status {
            UpdateStatus::Idle => {
                ui.label("No update check has run yet.");
            }
            UpdateStatus::Checking => {
                ui.label("Checking GitHub for a newer release…");
            }
            UpdateStatus::Available => {
                if let Some(info) = state.available.as_ref() {
                    ui.label(format!(
                        "Update available: {}",
                        info.TargetFullRelease.Version
                    ));
                    ui.label("The complete application package will be downloaded.");
                }
            }
            UpdateStatus::Downloading => {
                ui.label("Downloading update…");
            }
            UpdateStatus::ReadyToRestart => {
                ui.label("Update downloaded and ready to install.");
            }
            UpdateStatus::NotInstalled => {
                ui.label("Updates are available after installing a Velopack package.");
            }
            UpdateStatus::Error => {
                if let Some(error) = state.error.as_deref() {
                    ui.label(egui::RichText::new(error).color(egui::Color32::RED));
                }
            }
        }

        let mut actions = ctx.resource::<UpdateActions>().copied().unwrap_or_default();
        let original_actions = actions;
        ui.horizontal(|ui| {
            if state.status != UpdateStatus::Checking
                && state.status != UpdateStatus::Downloading
                && ui.button("Check now").clicked()
            {
                actions.check_requested = true;
            }
            if state.status == UpdateStatus::Available && ui.button("Download update").clicked() {
                actions.download_requested = true;
            }
            if state.status == UpdateStatus::ReadyToRestart
                && ui.button("Install and restart").clicked()
            {
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
    if !settings.auto_check || state.status != UpdateStatus::Idle || actions.check_requested {
        return;
    }
    let now = unix_now();
    if now.saturating_sub(settings.last_check_unix) < UPDATE_CHECK_INTERVAL_SECS {
        return;
    }
    settings.last_check_unix = now;
    actions.check_requested = true;
}

fn process_update_actions(
    mut actions: ResMut<UpdateActions>,
    mut state: ResMut<UpdateState>,
    mut tasks: ResMut<UpdateTasks>,
) {
    if actions.check_requested {
        actions.check_requested = false;
        if tasks.check.is_none() && tasks.download.is_none() {
            state.status = UpdateStatus::Checking;
            state.available = None;
            state.ready = None;
            state.error = None;
            tasks.check = Some(IoTaskPool::get().spawn(async { check_for_updates() }));
        }
    }

    if actions.download_requested {
        actions.download_requested = false;
        if tasks.check.is_none() && tasks.download.is_none() {
            if let Some(info) = state.available.clone() {
                state.status = UpdateStatus::Downloading;
                state.error = None;
                tasks.download =
                    Some(IoTaskPool::get().spawn(async move { download_update(info) }));
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
            }
            UpdateCheckResult::NoUpdate => {
                state.status = UpdateStatus::Idle;
                state.error = None;
            }
            UpdateCheckResult::NotInstalled => {
                state.status = UpdateStatus::NotInstalled;
                state.error = None;
            }
            UpdateCheckResult::Error(error) => {
                state.status = UpdateStatus::Error;
                state.error = Some(error);
            }
        }
    }

    let download_result = tasks
        .download
        .as_mut()
        .and_then(|task| future::block_on(future::poll_once(task)));
    if let Some(result) = download_result {
        tasks.download = None;
        match result.result {
            Ok(()) => {
                state.status = UpdateStatus::ReadyToRestart;
                state.ready = Some(result.info);
                state.error = None;
            }
            Err(error) => {
                state.status = UpdateStatus::Available;
                state.available = Some(result.info);
                state.error = Some(error);
            }
        }
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

fn download_update(info: UpdateInfo) -> UpdateDownloadResult {
    let result = create_update_manager()
        .and_then(|manager| manager.download_updates(&info, None))
        .map_err(|error| error.to_string());
    UpdateDownloadResult { info, result }
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
