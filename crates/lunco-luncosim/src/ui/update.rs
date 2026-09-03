//! Native Velopack update checks for the interactive desktop application.
//!
//! The updater is deliberately a UI concern.  The package itself is the
//! complete staged application directory, so Velopack applies the new package
//! as one unit: assets added by a release arrive, and files absent from the
//! release are not retained as an accidental second asset tree.

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bevy::prelude::*;
use bevy::tasks::{futures_lite::future, IoTaskPool, Task};
use bevy_egui::egui;
use lunco_settings::AppSettingsExt;
use lunco_workbench::status_bus::{StatusBarAction, StatusBus, StatusLevel};
use lunco_workbench::WorkbenchLayout;
use serde::{Deserialize, Serialize};
use velopack::sources::UpdateSource;
use velopack::{
    NetworkError, UpdateCheck, UpdateInfo, UpdateManager, VelopackAsset, VelopackAssetFeed,
};

const UPDATE_STATUS_SOURCE: &str = "updates";
/// Public machine-only repository containing immutable update releases.
pub(crate) const UPDATE_REPOSITORY: &str = "https://github.com/LunCoSim/lunco-sim-updates";
/// Bound each HTTP operation so a broken route cannot leave the UI in an
/// indefinite `Downloading` state. Range requests keep this per-chunk rather
/// than applying an unnecessarily short timeout to the complete 90 MB package.
const UPDATE_HTTP_TIMEOUT: Duration = Duration::from_secs(45);
const UPDATE_DOWNLOAD_CHUNK_BYTES: u64 = 1024 * 1024;
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
const UPDATE_PACKAGE_GUIDANCE: &str = "On Linux, run the official Velopack .AppImage from a writable location and keep launching that same file; the update replaces it in place.";
#[cfg(target_os = "windows")]
const UPDATE_PACKAGE_GUIDANCE: &str = "On Windows, install the official Setup.exe and launch the installed LunCoSim shortcut; updates replace the installed application and restart it.";
#[cfg(target_os = "macos")]
const UPDATE_PACKAGE_GUIDANCE: &str = "On macOS, install the official .pkg for your CPU and launch the installed LunCoSim.app; updates replace the app bundle and restart it.";
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
const UPDATE_PACKAGE_GUIDANCE: &str = "Run the official Velopack package from its installed location; source builds and ordinary archives are not update-managed.";

/// GitHub-backed source used by the native updater.
///
/// Velopack's built-in `GithubSource` uses an unbounded HTTP agent in the
/// pinned Rust release. That is unsafe for an interactive application: a
/// stalled route leaves its synchronous download worker alive forever, while
/// the UI can only observe the last progress value. This source keeps
/// Velopack's feed, package metadata, checksum, and apply machinery, but owns
/// the transport boundary so checks and package range requests have bounded
/// waits. The release repository contains full packages, so a one-megabyte
/// range also makes retrying a weak connection practical.
#[derive(Clone)]
struct TimeoutGithubSource {
    repository: &'static str,
    prerelease: bool,
    settings: lunco_settings::DownloadSettings,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    name: Option<String>,
    prerelease: bool,
    published_at: Option<String>,
    assets: Vec<GithubReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubReleaseAsset {
    url: Option<String>,
    browser_download_url: Option<String>,
    name: Option<String>,
}

impl TimeoutGithubSource {
    fn new(
        repository: &'static str,
        prerelease: bool,
        settings: lunco_settings::DownloadSettings,
    ) -> Self {
        Self {
            repository,
            prerelease,
            settings,
        }
    }

    fn releases_url(&self) -> String {
        let repository = self
            .repository
            .trim_end_matches('/')
            .strip_prefix("https://github.com/")
            .unwrap_or(self.repository.trim_end_matches('/'));
        format!("https://api.github.com/repos/{repository}/releases?per_page=10&page=1")
    }

    fn get_releases(&self) -> Result<Vec<GithubRelease>, velopack::Error> {
        let json = self.request_text(&self.releases_url(), "application/vnd.github.v3+json")?;
        let mut releases: Vec<GithubRelease> = serde_json::from_str(&json)?;
        releases.sort_by(|a, b| b.published_at.cmp(&a.published_at));
        if !self.prerelease {
            releases.retain(|release| !release.prerelease);
        }
        Ok(releases)
    }

    fn request_text(&self, url: &str, accept: &str) -> Result<String, velopack::Error> {
        let agent = update_http_agent();
        lunco_assets::download::retry_with_backoff(
            &self.settings,
            || {
                let mut response = agent.get(url).header("Accept", accept).call()?;
                Ok(response.body_mut().read_to_string()?)
            },
            is_retryable_update_error,
            || true,
        )
    }

    fn asset_url(release: &GithubRelease, asset_name: &str) -> Result<String, velopack::Error> {
        let release_name = release.name.as_deref().unwrap_or("unknown");
        let asset = release
            .assets
            .iter()
            .find(|asset| {
                asset
                    .name
                    .as_deref()
                    .is_some_and(|name| name.eq_ignore_ascii_case(asset_name))
            })
            .ok_or_else(|| {
                velopack::Error::Other(format!(
                    "Could not find asset called '{asset_name}' in GitHub Release '{release_name}'."
                ))
            })?;

        asset
            .browser_download_url
            .clone()
            .or_else(|| asset.url.clone())
            .ok_or_else(|| {
                velopack::Error::Other(
                    "Could not find a valid asset URL for the specified update.".to_owned(),
                )
            })
    }

    fn download_asset(
        &self,
        url: &str,
        size: u64,
        local_file: &Path,
        progress_sender: Option<mpsc::Sender<i16>>,
    ) -> Result<(), velopack::Error> {
        if size == 0 {
            return Err(velopack::Error::Other(
                "The update package has no advertised size.".to_owned(),
            ));
        }

        let agent = update_http_agent();
        // Velopack may call this source again with the same staging path after
        // a failed download. Keep complete ranges already written there and
        // resume at the next range instead of truncating the package back to
        // byte zero. A partial range is discarded and fetched again below.
        let existing_len = std::fs::metadata(local_file)
            .map(|metadata| metadata.len())
            .unwrap_or_default();
        let mut offset = if existing_len <= size {
            (existing_len / UPDATE_DOWNLOAD_CHUNK_BYTES) * UPDATE_DOWNLOAD_CHUNK_BYTES
        } else {
            0
        };
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .open(local_file)?;
        file.set_len(offset)?;
        file.seek(SeekFrom::Start(offset))?;
        while offset < size {
            let end = (offset + UPDATE_DOWNLOAD_CHUNK_BYTES - 1).min(size - 1);
            let bytes = download_range_with_policy(&agent, url, offset, end, &self.settings)?;
            let expected = end - offset + 1;
            if bytes.len() as u64 != expected {
                return Err(velopack::Error::Other(format!(
                    "The update server returned {}/{} bytes for range {offset}-{end}.",
                    bytes.len(),
                    expected
                )));
            }

            file.write_all(&bytes)?;
            offset = end + 1;
            if let Some(progress_sender) = &progress_sender {
                let progress = ((offset.saturating_mul(100) / size).min(100)) as i16;
                let _ = progress_sender.send(progress);
            }
        }
        file.flush()?;
        Ok(())
    }
}

impl UpdateSource for TimeoutGithubSource {
    fn get_release_feed(
        &self,
        channel: &str,
        _app: &velopack::bundle::Manifest,
        _staged_user_id: &str,
    ) -> Result<VelopackAssetFeed, velopack::Error> {
        let releases = self.get_releases()?;
        let feed_name = format!("releases.{channel}.json");
        let mut assets = Vec::new();
        let mut loaded_feed = false;

        for release in &releases {
            let Ok(url) = Self::asset_url(release, &feed_name) else {
                continue;
            };
            // A feed request that fails is not equivalent to an empty feed:
            // treating it as empty would tell the user that no update exists
            // precisely when the network hid the newest release.
            let json = self.request_text(&url, "application/octet-stream")?;
            let feed: VelopackAssetFeed = serde_json::from_str(&json)?;
            assets.extend(feed.Assets);
            loaded_feed = true;
        }

        if !loaded_feed {
            return Err(velopack::Error::Other(format!(
                "No {feed_name} feed was found in the update repository."
            )));
        }
        Ok(VelopackAssetFeed { Assets: assets })
    }

    fn download_release_entry(
        &self,
        asset: &VelopackAsset,
        local_file: &Path,
        progress_sender: Option<mpsc::Sender<i16>>,
    ) -> Result<(), velopack::Error> {
        let releases = self.get_releases()?;
        let url = releases
            .iter()
            .find_map(|release| Self::asset_url(release, &asset.FileName).ok())
            .ok_or_else(|| {
                velopack::Error::Other(format!(
                    "Could not find asset '{}' in any GitHub release.",
                    asset.FileName
                ))
            })?;
        self.download_asset(&url, asset.Size, local_file, progress_sender)
    }
}

fn update_http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(UPDATE_HTTP_TIMEOUT))
        .timeout_resolve(Some(UPDATE_HTTP_TIMEOUT))
        .timeout_connect(Some(UPDATE_HTTP_TIMEOUT))
        .timeout_recv_response(Some(UPDATE_HTTP_TIMEOUT))
        .timeout_recv_body(Some(UPDATE_HTTP_TIMEOUT))
        .build()
        .into()
}

fn download_range_with_policy(
    agent: &ureq::Agent,
    url: &str,
    start: u64,
    end: u64,
    settings: &lunco_settings::DownloadSettings,
) -> Result<Vec<u8>, velopack::Error> {
    let mut bytes = Vec::new();
    lunco_assets::download::retry_with_backoff(
        settings,
        || {
            let next_start = start.saturating_add(bytes.len() as u64);
            if next_start > end {
                return Ok(());
            }
            download_range_into(agent, url, next_start, end, &mut bytes)
        },
        is_retryable_update_error,
        || true,
    )?;
    Ok(bytes)
}

fn is_retryable_update_error(error: &velopack::Error) -> bool {
    match error {
        velopack::Error::Network(network) => match network.as_ref() {
            NetworkError::Http(error) => lunco_assets::download::is_retryable_download_error(error),
            NetworkError::Url(_) => false,
        },
        velopack::Error::Io(_) => true,
        _ => false,
    }
}

fn download_range_into(
    agent: &ureq::Agent,
    url: &str,
    start: u64,
    end: u64,
    bytes: &mut Vec<u8>,
) -> Result<(), velopack::Error> {
    let range = format!("bytes={start}-{end}");
    let mut response = agent
        .get(url)
        .header("Accept", "application/octet-stream")
        .header("Range", &range)
        .call()?;
    if response.status().as_u16() != 206 {
        return Err(velopack::Error::Other(format!(
            "The update server did not honour range {range} (HTTP {}).",
            response.status().as_u16()
        )));
    }
    let valid_start = response
        .headers()
        .get("content-range")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("bytes "))
        .and_then(|value| value.split_once('-'))
        .and_then(|(start, _)| start.parse::<u64>().ok());
    if valid_start != Some(start) {
        return Err(velopack::Error::Other(format!(
            "The update server returned an invalid Content-Range for {range}."
        )));
    }
    response.body_mut().as_reader().read_to_end(bytes)?;
    Ok(())
}

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
        lunco_settings::ensure_download_settings(app);
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
        UpdateStatus::Error => actions.check_requested = true,
        UpdateStatus::Checking
        | UpdateStatus::Downloading
        | UpdateStatus::Idle
        | UpdateStatus::NotInstalled => {}
    }
}

fn register_update_settings_menu(world: &mut World) {
    let Some(mut layout) = world.get_resource_mut::<WorkbenchLayout>() else {
        return;
    };
    layout.register_settings_submenu("Updates", |ui, ctx| {
        ui.label(egui::RichText::new("Velopack updates").weak().small());
        let identity = ctx.resource::<lunco_workbench::BuildIdentity>();
        egui::Grid::new("updates_build_identity")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label("Version");
                ui.monospace(
                    identity
                        .map(|identity| identity.version.as_str())
                        .unwrap_or("Unavailable"),
                );
                ui.end_row();

                ui.label("GitHub Actions build");
                ui.monospace(github_build_label(
                    identity.map(|identity| identity.version.as_str()),
                ));
                ui.end_row();

                ui.label("Update channel");
                ui.monospace(UPDATE_CHANNEL);
                ui.end_row();
            });
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
            if can_check
                && lunco_workbench::icon_text_button(
                    ui,
                    lunco_workbench::UiIcon::Refresh,
                    "Check now",
                    "Check GitHub for a newer LunCoSim release",
                )
                .clicked()
            {
                actions.check_requested = true;
            }
            let download_label = if state.error.is_some() {
                "Retry download"
            } else {
                "Download update"
            };
            let download_icon = if state.error.is_some() {
                lunco_workbench::UiIcon::Refresh
            } else {
                lunco_workbench::UiIcon::Download
            };
            if state.status == UpdateStatus::Available
                && lunco_workbench::icon_text_button(
                    ui,
                    download_icon,
                    download_label,
                    "Download the selected LunCoSim update",
                )
                .clicked()
            {
                actions.download_requested = true;
            }
            if state.ready.is_some()
                && lunco_workbench::icon_text_button(
                    ui,
                    lunco_workbench::UiIcon::Play,
                    "Restart to install",
                    "Install the downloaded LunCoSim update",
                )
                .clicked()
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
    settings: Res<lunco_settings::DownloadSettings>,
) {
    let settings = settings.clone();
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
            let retry_settings = settings.clone();
            tasks.check =
                Some(IoTaskPool::get().spawn(async move { check_for_updates(retry_settings) }));
        }
    }

    if actions.download_requested {
        actions.download_requested = false;
        if tasks.check.is_none() && tasks.download.is_none() {
            if let Some(info) = state.available.clone() {
                let (progress_sender, progress_receiver) = mpsc::channel();
                state.status = UpdateStatus::Downloading;
                state.error = None;
                // No bytes have arrived yet. Keep this indeterminate instead
                // of presenting a misleading frozen 0% bar while the first
                // bounded range request is connecting.
                state.download_progress = None;
                tasks.download_progress = Some(Arc::new(Mutex::new(progress_receiver)));
                let retry_settings = settings.clone();
                tasks.download =
                    Some(IoTaskPool::get().spawn(async move {
                        download_update(info, progress_sender, retry_settings)
                    }));
            }
        }
    }

    if actions.apply_requested {
        actions.apply_requested = false;
        let Some(info) = state.ready.clone() else {
            return;
        };
        match create_update_manager(&settings) {
            Ok(manager) => {
                if let Err(error) = manager.apply_updates_and_restart(&info) {
                    state.status = UpdateStatus::Error;
                    state.error = Some(user_facing_update_error("install the update", error));
                }
            }
            Err(error) => {
                state.status = UpdateStatus::Error;
                state.error = Some(user_facing_update_error("prepare the update", error));
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
                    if let Some(progress) = state.download_progress {
                        ui.label(format!("Downloading update… {progress}%"));
                        ui.add_sized(
                            [ui.available_width(), 24.0],
                            egui::ProgressBar::new(f32::from(progress) / 100.0)
                                .show_percentage(),
                        );
                    } else {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Connecting to the update server…");
                        });
                    }
                    ui.label(
                        "You can keep working. If the connection fails, LunCoSim will keep the current version and offer Retry download.",
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
            let version = state
                .available
                .as_ref()
                .map(|info| info.TargetFullRelease.Version.as_str())
                .unwrap_or("update");
            if let Some(progress) = state.download_progress {
                let progress = u64::from(progress);
                bus.set_progress(
                    UPDATE_STATUS_SOURCE,
                    format!("downloading {version}… {progress}%"),
                    progress,
                    100,
                );
            } else {
                bus.set_progress(
                    UPDATE_STATUS_SOURCE,
                    format!("connecting to update server for {version}…"),
                    0,
                    0,
                );
            }
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
                        StatusLevel::Attention,
                        format!("update check failed: {error} · Check again"),
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

fn check_for_updates(settings: lunco_settings::DownloadSettings) -> UpdateCheckResult {
    let manager = match create_update_manager(&settings) {
        Ok(manager) => manager,
        Err(velopack::Error::NotInstalled(_)) => return UpdateCheckResult::NotInstalled,
        Err(error) => {
            return UpdateCheckResult::Error(user_facing_update_error("check for updates", error))
        }
    };
    match manager.check_for_updates() {
        Ok(UpdateCheck::UpdateAvailable(info)) => UpdateCheckResult::Available(info),
        Ok(UpdateCheck::NoUpdateAvailable | UpdateCheck::RemoteIsEmpty) => {
            UpdateCheckResult::NoUpdate
        }
        Err(error) => {
            UpdateCheckResult::Error(user_facing_update_error("check for updates", error))
        }
    }
}

fn download_update(
    info: UpdateInfo,
    progress: mpsc::Sender<i16>,
    settings: lunco_settings::DownloadSettings,
) -> UpdateDownloadResult {
    let result = create_update_manager(&settings)
        .and_then(|manager| manager.download_updates(&info, Some(progress)))
        .map_err(|error| user_facing_update_error("download the update", error));
    UpdateDownloadResult { info, result }
}

fn clamp_download_progress(progress: i16) -> u8 {
    progress.clamp(0, 100) as u8
}

fn create_update_manager(
    settings: &lunco_settings::DownloadSettings,
) -> Result<UpdateManager, velopack::Error> {
    let source = TimeoutGithubSource::new(UPDATE_REPOSITORY, true, settings.clone());
    UpdateManager::new(
        source,
        Some(velopack::UpdateOptions {
            ExplicitChannel: Some(UPDATE_CHANNEL.to_string()),
            ..Default::default()
        }),
        None,
    )
}

fn user_facing_update_error(action: &str, error: velopack::Error) -> String {
    match error {
        velopack::Error::Network(_) => {
            "Could not reach the update service. Check your internet connection and try again. The current LunCoSim version is still safe to use.".to_owned()
        }
        velopack::Error::ChecksumInvalid(..) | velopack::Error::SizeInvalid(..) => {
            "The update download was incomplete or corrupted. No update was installed; try again.".to_owned()
        }
        velopack::Error::Io(error) => format!(
            "Could not {action} because of a local file error: {error}. Check disk space and permissions, then try again."
        ),
        velopack::Error::Json(_) => {
            "The update service returned invalid release data. Try again later.".to_owned()
        }
        error => format!("Could not {action}: {error}"),
    }
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

fn github_build_label(version: Option<&str>) -> String {
    let Some(version) = version else {
        return "Unavailable (local build)".to_owned();
    };
    let Some((_, nightly)) = version.rsplit_once("-nightly.") else {
        return "Unavailable (local build)".to_owned();
    };
    let mut components = nightly.split('.');
    let Some(run) = components.next().filter(|value| !value.is_empty()) else {
        return "Unavailable (invalid release version)".to_owned();
    };
    let Some(attempt) = components.next().filter(|value| !value.is_empty()) else {
        return "Unavailable (invalid release version)".to_owned();
    };
    if components.next().is_some() || run.parse::<u64>().is_err() || attempt.parse::<u64>().is_err()
    {
        return "Unavailable (invalid release version)".to_owned();
    }
    format!("Run #{run}, attempt {attempt}")
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
    fn github_build_label_uses_ci_run_metadata_only() {
        assert_eq!(
            github_build_label(Some("0.6.0-nightly.37.1")),
            "Run #37, attempt 1"
        );
        assert_eq!(
            github_build_label(Some("0.6.0-dev")),
            "Unavailable (local build)"
        );
        assert_eq!(github_build_label(None), "Unavailable (local build)");
        assert_eq!(
            github_build_label(Some("0.6.0-nightly.bad.1")),
            "Unavailable (invalid release version)"
        );
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

    #[test]
    fn status_bar_click_retries_a_failed_update_check() {
        let state = UpdateState {
            status: UpdateStatus::Error,
            error: Some("network unavailable".to_owned()),
            ..Default::default()
        };
        let mut actions = UpdateActions::default();

        queue_status_bar_action(UPDATE_STATUS_SOURCE, &state, &mut actions);

        assert!(actions.check_requested);
        assert!(!actions.download_requested);
        assert!(!actions.apply_requested);
    }

    #[test]
    fn timeout_source_targets_machine_feed_with_bounded_range_downloads() {
        let settings = lunco_settings::DownloadSettings::default();
        let source = TimeoutGithubSource::new(UPDATE_REPOSITORY, true, settings.clone());

        assert_eq!(
            source.releases_url(),
            "https://api.github.com/repos/LunCoSim/lunco-sim-updates/releases?per_page=10&page=1"
        );
        assert_eq!(UPDATE_DOWNLOAD_CHUNK_BYTES, 1024 * 1024);
        assert!(UPDATE_HTTP_TIMEOUT.as_secs() > 0);
        assert_eq!(source.settings.max_attempts, settings.max_attempts);
    }

    #[test]
    fn range_download_requests_and_returns_only_the_selected_bytes() {
        use std::io::Read as _;
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("read test server address");
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept range request");
            let mut request = Vec::new();
            loop {
                let mut chunk = [0_u8; 256];
                let length = stream.read(&mut chunk).expect("read range request");
                request.extend_from_slice(&chunk[..length]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&request).to_ascii_lowercase();
            assert!(request.contains("range: bytes=3-6"));
            std::io::Write::write_all(
                &mut stream,
                b"HTTP/1.1 206 Partial Content\r\nContent-Length: 4\r\nContent-Range: bytes 3-6/10\r\n\r\n3456",
            )
            .expect("write range response");
        });

        let bytes = download_range_with_policy(
            &update_http_agent(),
            &format!("http://{address}"),
            3,
            6,
            &lunco_settings::DownloadSettings {
                max_attempts: 1,
                retry_initial_delay_secs: 0,
                ..Default::default()
            },
        )
        .expect("range request succeeds");
        assert_eq!(bytes, b"3456");
    }

    #[test]
    fn range_download_resumes_a_truncated_chunk() {
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("read test server address");
        let server = std::thread::spawn(move || {
            for expected_range in ["bytes=3-6", "bytes=5-6"] {
                let (mut stream, _) = listener.accept().expect("accept range request");
                let mut request = Vec::new();
                loop {
                    let mut chunk = [0_u8; 256];
                    let length = stream.read(&mut chunk).expect("read range request");
                    request.extend_from_slice(&chunk[..length]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&request).to_ascii_lowercase();
                assert!(request.contains(&format!("range: {expected_range}")));
                if expected_range == "bytes=3-6" {
                    stream
                        .write_all(
                            b"HTTP/1.1 206 Partial Content\r\nContent-Length: 4\r\nContent-Range: bytes 3-6/10\r\n\r\n34",
                        )
                        .expect("write truncated range response");
                } else {
                    stream
                        .write_all(
                            b"HTTP/1.1 206 Partial Content\r\nContent-Length: 2\r\nContent-Range: bytes 5-6/10\r\n\r\n56",
                        )
                        .expect("write resumed range response");
                }
            }
        });

        let bytes = download_range_with_policy(
            &update_http_agent(),
            &format!("http://{address}"),
            3,
            6,
            &lunco_settings::DownloadSettings {
                max_attempts: 2,
                retry_initial_delay_secs: 0,
                ..Default::default()
            },
        )
        .expect("truncated range resumes from its received prefix");
        server.join().expect("resume server completed");
        assert_eq!(bytes, b"3456");
    }
}
