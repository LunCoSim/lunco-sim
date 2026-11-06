//! Scene-loading settings shared by workspace opens and the settings UI.

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use lunco_settings::SettingsSection;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Twin manifest key for the automatic-loading override.
pub const SCENE_LOAD_DEFAULT_KEY: &str = "scene.load_default";
const SCENE_SECTION: &str = "scene";
const LOAD_DEFAULT_FIELD: &str = "load_default";

/// Global preference for automatic Twin scene loading.
#[derive(Resource, Reflect, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[reflect(Resource)]
#[serde(default)]
pub struct SceneLoadingSettings {
    /// Load the authored default when opening a Twin without an explicit scene.
    pub load_default: bool,
}

impl Default for SceneLoadingSettings {
    fn default() -> Self {
        Self { load_default: true }
    }
}

impl SettingsSection for SceneLoadingSettings {
    const KEY: &'static str = SCENE_SECTION;
}

/// Project-local overrides; absent fields inherit the global value.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectSceneSettings {
    /// Override the automatic loading preference for this project.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_default: Option<bool>,
}

impl SettingsSection for ProjectSceneSettings {
    const KEY: &'static str = SCENE_SECTION;
}

/// Process-wide command-line override. It is never persisted.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneLoadingCli(pub Option<bool>);

/// Parse the scene-loading flags, rejecting contradictory choices.
pub fn parse_scene_loading_cli(args: &[String]) -> Result<SceneLoadingCli, String> {
    let mut value = None;
    for arg in args {
        let next = match arg.as_str() {
            "--load-default-scene" => true,
            "--no-load-default-scene" => false,
            _ => continue,
        };
        if value.is_some_and(|previous| previous != next) {
            return Err(
                "--load-default-scene and --no-load-default-scene cannot be combined".into(),
            );
        }
        value = Some(next);
    }
    Ok(SceneLoadingCli(value))
}

/// The scope which supplied an effective value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneLoadingSource {
    Global,
    Project,
    Twin,
    Cli,
}

impl SceneLoadingSource {
    /// Stable name exposed to policies and presentation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
            Self::Twin => "twin",
            Self::Cli => "cli",
        }
    }
}

/// The effective automatic-loading value and its source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SceneLoadingDecision {
    /// Whether to load the authored default when no explicit scene was requested.
    pub load_default: bool,
    /// Scope which supplied this decision.
    pub source: SceneLoadingSource,
}

/// Resolve the declared order without conflating false with absence.
pub fn resolve_scene_loading(
    global: bool,
    project: Option<bool>,
    twin: Option<bool>,
    cli: Option<bool>,
) -> SceneLoadingDecision {
    let (load_default, source) = if let Some(value) = cli {
        (value, SceneLoadingSource::Cli)
    } else if let Some(value) = twin {
        (value, SceneLoadingSource::Twin)
    } else if let Some(value) = project {
        (value, SceneLoadingSource::Project)
    } else {
        (global, SceneLoadingSource::Global)
    };
    SceneLoadingDecision {
        load_default,
        source,
    }
}

/// Validate and read the Twin's optional automatic-loading override.
pub fn twin_scene_loading_setting(
    manifest: Option<&lunco_twin::TwinManifest>,
) -> Result<Option<bool>, String> {
    match manifest.and_then(|manifest| manifest.setting(SCENE_LOAD_DEFAULT_KEY)) {
        None => Ok(None),
        Some(lunco_twin::TwinSettingValue::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(format!("{SCENE_LOAD_DEFAULT_KEY} must be a boolean")),
    }
}

/// Canonical local settings file for a project root.
pub fn project_settings_path(root: &Path) -> PathBuf {
    root.join(".lunco").join("settings.json")
}

/// Read project overrides. Must run on a worker.
pub fn read_project_scene_settings(root: &Path) -> Result<ProjectSceneSettings, String> {
    lunco_settings::read_optional_section::<ProjectSceneSettings>(&project_settings_path(root))
        .map(|section| section.unwrap_or_default())
}

/// Request a local project override; `None` restores inheritance.
#[lunco_core::Command(default)]
pub struct SetProjectSceneLoading {
    /// Twin whose project settings are being edited.
    pub twin: u64,
    /// Explicit override or inheritance.
    pub value: Option<bool>,
}

struct ProjectWrite {
    twin: crate::TwinId,
    root: PathBuf,
    value: Option<bool>,
    task: Task<Result<(), String>>,
    revision: std::sync::Arc<std::sync::atomic::AtomicU64>,
    command_id: Option<u64>,
}

/// Outstanding project settings writes. Each root permits one write at a time.
#[derive(Resource, Default)]
pub struct PendingProjectSceneWrites {
    writes: Vec<ProjectWrite>,
    admitted_commands: std::collections::BTreeSet<u64>,
}

impl PendingProjectSceneWrites {
    pub(crate) fn has_pending(&self) -> bool {
        !self.writes.is_empty()
    }

    /// Whether a reflected command entered the asynchronous write boundary.
    pub fn was_admitted(&self, id: u64) -> bool {
        self.admitted_commands.contains(&id)
    }

    /// Retire the API adapter's completed command identity.
    pub fn forget_admitted(&mut self, id: u64) {
        self.admitted_commands.remove(&id);
    }
}

#[lunco_core::on_command(SetProjectSceneLoading)]
fn set_project_scene_loading(
    trigger: On<SetProjectSceneLoading>,
    mut workspace: ResMut<crate::WorkspaceResource>,
    mut pending: ResMut<PendingProjectSceneWrites>,
    opens: Res<crate::open::PendingTwinOpens>,
    active: Res<lunco_core::ActiveCommandId>,
) -> lunco_core::CommandOutcome {
    let id = crate::TwinId::new(trigger.event().twin);
    let Some(twin) = workspace.twin(id) else {
        let error = format!(
            "Cannot update project settings: Twin {} is no longer open",
            id.raw()
        );
        warn!("{error}");
        return lunco_core::CommandOutcome::Rejected(lunco_command_contracts::Reject::InvalidOp(
            error,
        ));
    };
    if twin.manifest.is_none() {
        let error = "Cannot update project settings: no authored Twin is open".to_owned();
        warn!("{error}");
        return lunco_core::CommandOutcome::Rejected(lunco_command_contracts::Reject::InvalidOp(
            error,
        ));
    }
    let root = twin.root.clone();
    if pending.writes.iter().any(|write| write.root == root) {
        workspace.project_settings_errors.insert(
            id,
            "A project settings update is already in progress".into(),
        );
        warn!(
            "A project settings update is already in progress for {}",
            root.display()
        );
        return lunco_core::CommandOutcome::Rejected(lunco_command_contracts::Reject::InvalidOp(
            format!(
                "A project settings update is already in progress for {}",
                root.display()
            ),
        ));
    }
    let value = trigger.event().value;
    let path = project_settings_path(&root);
    let revision = opens.settings_revision.clone();
    revision.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    let task = AsyncComputeTaskPool::get().spawn(async move {
        lunco_settings::read_optional_section::<ProjectSceneSettings>(&path)?;
        lunco_settings::write_optional_field(
            &path,
            SCENE_SECTION,
            LOAD_DEFAULT_FIELD,
            value.as_ref(),
        )
    });
    workspace.project_settings_errors.remove(&id);
    workspace.project_settings_pending.insert(id);
    let command_id = active.get();
    if let Some(command_id) = command_id {
        pending.admitted_commands.insert(command_id);
        while pending.admitted_commands.len() > 1024 {
            pending.admitted_commands.pop_first();
        }
    }
    pending.writes.push(ProjectWrite {
        twin: id,
        root,
        value,
        task,
        revision,
        command_id,
    });
    lunco_core::CommandOutcome::Pending
}

fn poll_project_scene_writes(
    mut pending: ResMut<PendingProjectSceneWrites>,
    mut workspace: ResMut<crate::WorkspaceResource>,
    mut results: ResMut<lunco_core::CommandResults>,
) {
    use bevy::tasks::futures_lite::future;
    pending.writes.retain_mut(|write| {
        let Some(result) = future::block_on(future::poll_once(&mut write.task)) else {
            return true;
        };
        write
            .revision
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        if let Some(id) = write.command_id {
            results.record(
                id,
                result
                    .as_ref()
                    .map(|()| lunco_command_contracts::Ack::default())
                    .map_err(Clone::clone),
            );
        }
        if result.is_ok() {
            let ids: Vec<_> = workspace
                .twins()
                .filter(|(_, twin)| twin.root == write.root)
                .map(|(id, _)| id)
                .collect();
            for id in ids {
                if let Some(project) = workspace.project_scene_settings.get_mut(&id) {
                    project.load_default = write.value;
                }
            }
        }
        if workspace
            .twin(write.twin)
            .is_some_and(|twin| twin.root == write.root)
        {
            workspace.project_settings_pending.remove(&write.twin);
            match result {
                Ok(()) => {
                    workspace.project_settings_errors.remove(&write.twin);
                }
                Err(error) => {
                    warn!("Project settings write failed: {error}");
                    workspace.project_settings_errors.insert(write.twin, error);
                }
            }
        } else if let Err(error) = result {
            warn!("Project settings write failed after Twin closed: {error}");
        }
        false
    });
}

fn finish_project_writes_before_exit(world: &mut World) {
    let exiting = world
        .get_resource::<bevy::ecs::message::Messages<bevy::app::AppExit>>()
        .is_some_and(|messages| !messages.is_empty());
    if !exiting {
        return;
    }
    let writes = std::mem::take(&mut world.resource_mut::<PendingProjectSceneWrites>().writes);
    for write in writes {
        if let Err(error) = bevy::tasks::block_on(write.task) {
            warn!("Project settings write failed during exit: {error}");
        }
        write
            .revision
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }
}

fn project_scene_writes_pending(pending: Res<PendingProjectSceneWrites>) -> bool {
    pending.has_pending()
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<PendingProjectSceneWrites>()
        .add_systems(
            Update,
            poll_project_scene_writes.run_if(project_scene_writes_pending),
        )
        .add_systems(Last, finish_project_writes_before_exit);
    register_all_commands(app);
}

lunco_core::register_commands!(set_project_scene_loading);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_loading_values_preserve_inheritance_and_unrelated_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = project_settings_path(dir.path());
        assert_eq!(
            read_project_scene_settings(dir.path())
                .unwrap()
                .load_default,
            None
        );
        lunco_storage::write_file_sync(
            &path,
            br#"{"scene":{"other":7},"unrelated":{"keep":true}}"#,
        )
        .unwrap();
        lunco_settings::write_optional_field(&path, "scene", "load_default", Some(&false)).unwrap();
        assert_eq!(
            read_project_scene_settings(dir.path())
                .unwrap()
                .load_default,
            Some(false)
        );
        let stored: serde_json::Value =
            serde_json::from_slice(&lunco_storage::read_file_sync(&path).unwrap()).unwrap();
        assert_eq!(stored["scene"]["other"], 7);
        assert_eq!(stored["unrelated"]["keep"], true);
        let project = Some(false);
        assert_eq!(
            resolve_scene_loading(true, project, None, None),
            SceneLoadingDecision {
                load_default: false,
                source: SceneLoadingSource::Project
            }
        );
        assert_eq!(
            resolve_scene_loading(true, project, Some(true), None).source,
            SceneLoadingSource::Twin
        );
        assert_eq!(
            resolve_scene_loading(true, project, Some(true), Some(false)),
            SceneLoadingDecision {
                load_default: false,
                source: SceneLoadingSource::Cli
            }
        );
        lunco_settings::write_optional_field::<bool>(&path, "scene", "load_default", None).unwrap();
        assert_eq!(
            read_project_scene_settings(dir.path())
                .unwrap()
                .load_default,
            None
        );
        let invalid = br#"{"scene":{"load_default":"wrong"}}"#;
        lunco_storage::write_file_sync(&path, invalid).unwrap();
        assert!(
            read_project_scene_settings(dir.path())
                .unwrap_err()
                .contains("expected a boolean")
        );
        assert_eq!(lunco_storage::read_file_sync(&path).unwrap(), invalid);
        assert!(
            parse_scene_loading_cli(&[
                "--load-default-scene".into(),
                "--no-load-default-scene".into()
            ])
            .is_err()
        );
    }
}
