//! Policy-owned dataset consent and progress presentation.
//!
//! The dataset registry and downloader expose typed facts and generic commands.
//! Rhai decides whether a scope is visible, which rows/actions exist, and what
//! each mode means. The UI only validates the returned view model and publishes
//! it to the generic HTML/CSS runtime surface.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use lunco_assets_datasets::{
    DatasetRegistry, DatasetScope, DatasetScopeReady, DatasetScopeRemoved, DatasetState,
};
use lunco_core::{on_command, register_commands, Command};
use lunco_exposure_core::{EngineExposures, ExposureValue};
use lunco_hooks::HookValue;
use lunco_workspace::WorkspaceResource;

const SURFACE_NAMESPACE: &str = "asset-consent";
const MAX_PROVISIONING_ROWS: usize = 256;
const MAX_PROVISIONING_ACTIONS: usize = 32;

#[derive(Resource, Default)]
pub(crate) struct DatasetProvisioningState {
    pending: Vec<DatasetScope>,
    active: Option<DatasetScope>,
    dismissed: HashSet<String>,
    last_hook_generation: u64,
}

#[derive(Clone, Debug)]
struct ProvisioningRow {
    id: String,
    parameter_id: String,
    label: String,
    status: String,
    action: String,
    action_label: String,
}

#[derive(Clone, Debug)]
struct ProvisioningAction {
    key: String,
    label: String,
    action: String,
    scope: String,
}

#[derive(Clone, Debug)]
struct ProvisioningView {
    visible: bool,
    title: String,
    description: String,
    scope_label: String,
    rows: Vec<ProvisioningRow>,
    actions: Vec<ProvisioningAction>,
}

fn scope_id(scope: &DatasetScope) -> String {
    match scope {
        DatasetScope::Engine => "engine".to_owned(),
        DatasetScope::Twin { name, .. } => format!("twin:{name}"),
    }
}

fn scope_kind(scope: &DatasetScope) -> &'static str {
    match scope {
        DatasetScope::Engine => "engine",
        DatasetScope::Twin { .. } => "twin",
    }
}

fn needs_provisioning(state: &DatasetState) -> bool {
    matches!(
        state,
        DatasetState::Missing | DatasetState::Failed(_) | DatasetState::Cancelled
    )
}

fn recommended_missing(registry: &DatasetRegistry, scope: &DatasetScope) -> bool {
    registry
        .entries()
        .iter()
        .any(|entry| &entry.scope == scope && entry.recommended && needs_provisioning(&entry.state))
}

fn same_scope(state: &DatasetProvisioningState, scope: &DatasetScope) -> bool {
    state.pending.iter().any(|candidate| candidate == scope)
        || state
            .active
            .as_ref()
            .is_some_and(|candidate| candidate == scope)
}

fn map_fields<'a>(
    value: &'a HookValue,
    context: &str,
) -> Result<&'a [(String, HookValue)], String> {
    match value {
        HookValue::Map(fields) => Ok(fields),
        value => Err(format!(
            "{context} must be a map, got {}",
            value.type_name()
        )),
    }
}

fn field<'a>(
    fields: &'a [(String, HookValue)],
    name: &str,
    context: &str,
) -> Result<&'a HookValue, String> {
    let matches = fields
        .iter()
        .filter(|(key, _)| key == name)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [pair] => Ok(&pair.1),
        [] => Err(format!("{context} is missing `{name}`")),
        _ => Err(format!("{context} contains `{name}` more than once")),
    }
}

fn reject_unknown(
    fields: &[(String, HookValue)],
    allowed: &[&str],
    context: &str,
) -> Result<(), String> {
    for (key, _) in fields {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("{context} contains unknown field `{key}`"));
        }
    }
    Ok(())
}

fn string_field(
    fields: &[(String, HookValue)],
    name: &str,
    context: &str,
) -> Result<String, String> {
    field(fields, name, context)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{context}.{name} must be a string"))
}

fn bool_field(fields: &[(String, HookValue)], name: &str, context: &str) -> Result<bool, String> {
    match field(fields, name, context)? {
        HookValue::Bool(value) => Ok(*value),
        value => Err(format!(
            "{context}.{name} must be a bool, got {}",
            value.type_name()
        )),
    }
}

fn array_field<'a>(
    fields: &'a [(String, HookValue)],
    name: &str,
    context: &str,
) -> Result<&'a [HookValue], String> {
    match field(fields, name, context)? {
        HookValue::Array(values) => Ok(values),
        value => Err(format!(
            "{context}.{name} must be an array, got {}",
            value.type_name()
        )),
    }
}

fn parse_view(value: HookValue) -> Result<ProvisioningView, String> {
    let fields = map_fields(&value, "assets.provision result")?;
    reject_unknown(
        fields,
        &[
            "visible",
            "title",
            "description",
            "scope_label",
            "rows",
            "actions",
        ],
        "assets.provision result",
    )?;
    let visible = bool_field(fields, "visible", "assets.provision result")?;
    let title = string_field(fields, "title", "assets.provision result")?;
    let description = string_field(fields, "description", "assets.provision result")?;
    let scope_label = string_field(fields, "scope_label", "assets.provision result")?;
    let row_values = array_field(fields, "rows", "assets.provision result")?;
    if row_values.len() > MAX_PROVISIONING_ROWS {
        return Err(format!(
            "assets.provision result.rows exceeds the {MAX_PROVISIONING_ROWS}-row limit"
        ));
    }
    let rows = row_values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let context = format!("assets.provision.rows[{index}]");
            let fields = map_fields(value, &context)?;
            reject_unknown(
                fields,
                &[
                    "id",
                    "parameter_id",
                    "label",
                    "status",
                    "action",
                    "action_label",
                ],
                &context,
            )?;
            Ok(ProvisioningRow {
                id: string_field(fields, "id", &context)?,
                parameter_id: string_field(fields, "parameter_id", &context)?,
                label: string_field(fields, "label", &context)?,
                status: string_field(fields, "status", &context)?,
                action: string_field(fields, "action", &context)?,
                action_label: string_field(fields, "action_label", &context)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let action_values = array_field(fields, "actions", "assets.provision result")?;
    if action_values.len() > MAX_PROVISIONING_ACTIONS {
        return Err(format!(
            "assets.provision result.actions exceeds the {MAX_PROVISIONING_ACTIONS}-action limit"
        ));
    }
    let actions = action_values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let context = format!("assets.provision.actions[{index}]");
            let fields = map_fields(value, &context)?;
            reject_unknown(fields, &["key", "label", "action", "scope"], &context)?;
            Ok(ProvisioningAction {
                key: string_field(fields, "key", &context)?,
                label: string_field(fields, "label", &context)?,
                action: string_field(fields, "action", &context)?,
                scope: string_field(fields, "scope", &context)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut row_ids = HashSet::new();
    for row in &rows {
        if row.id.trim().is_empty() || row.parameter_id.trim().is_empty() {
            return Err("assets.provision rows require non-empty ids".to_owned());
        }
        if !row_ids.insert(&row.id) {
            return Err(format!("assets.provision rows repeat id `{}`", row.id));
        }
        if row.action.trim().is_empty() || row.action_label.trim().is_empty() {
            return Err(format!(
                "assets.provision row `{}` requires an action and action label",
                row.id
            ));
        }
    }
    let mut action_keys = HashSet::new();
    for action in &actions {
        if action.key.trim().is_empty()
            || action.action.trim().is_empty()
            || action.label.trim().is_empty()
            || action.scope.trim().is_empty()
        {
            return Err("assets.provision actions require non-empty fields".to_owned());
        }
        if !action_keys.insert(&action.key) {
            return Err(format!(
                "assets.provision actions repeat key `{}`",
                action.key
            ));
        }
    }
    Ok(ProvisioningView {
        visible,
        title,
        description,
        scope_label,
        rows,
        actions,
    })
}

fn state_value(state: &DatasetState) -> HookValue {
    let kind = match state {
        DatasetState::Missing => "missing",
        DatasetState::Downloading { .. } => "downloading",
        DatasetState::Processing { .. } => "processing",
        DatasetState::Cancelling => "cancelling",
        DatasetState::Installed => "installed",
        DatasetState::Cancelled => "cancelled",
        DatasetState::Failed(_) => "failed",
    };
    HookValue::map([
        ("kind", HookValue::str(kind)),
        (
            "detail",
            match state {
                DatasetState::Failed(error) => HookValue::str(error.clone()),
                _ => HookValue::Unit,
            },
        ),
    ])
}

fn invoke_policy(
    registry: &DatasetRegistry,
    scope: &DatasetScope,
    workspace: Option<&WorkspaceResource>,
    interactive: bool,
) -> Result<ProvisioningView, String> {
    let root = project_root_for_scope(scope, workspace);
    let suppressed = project_suppresses_prompt(root.as_deref(), workspace);
    let datasets = registry
        .entries()
        .iter()
        .filter(|entry| &entry.scope == scope)
        .map(|entry| {
            HookValue::map([
                ("id", HookValue::str(entry.id.clone())),
                ("key", HookValue::str(entry.key.clone())),
                ("name", HookValue::str(entry.name.clone())),
                ("recommended", HookValue::Bool(entry.recommended)),
                ("processed", HookValue::Bool(entry.spec.process.is_some())),
                ("state", state_value(&entry.state)),
            ])
        })
        .collect();
    let facts = HookValue::map([
        ("scope", HookValue::str(scope_kind(scope))),
        ("scope_id", HookValue::str(scope_id(scope))),
        ("owner", HookValue::str(scope.label())),
        ("interactive", HookValue::Bool(interactive)),
        ("show_on_start", HookValue::Bool(!suppressed)),
        ("suppressed", HookValue::Bool(suppressed)),
        ("suppressible", HookValue::Bool(root.is_some())),
        ("datasets", HookValue::Array(datasets)),
    ]);
    match lunco_hooks::invoke(lunco_core_session::DATASET_PROVISION_HOOK, &[facts]) {
        Some(Ok(value)) => parse_view(value),
        Some(Err(error)) => Err(format!("assets.provision failed: {error}")),
        None => Err("assets.provision is not installed".to_owned()),
    }
}

fn exposure_map(view: &ProvisioningView) -> ExposureValue {
    ExposureValue::Map(vec![
        (
            "rows".to_owned(),
            ExposureValue::Array(
                view.rows
                    .iter()
                    .map(|row| {
                        ExposureValue::Map(vec![
                            ("id".to_owned(), row.id.clone().into()),
                            ("parameter_id".to_owned(), row.parameter_id.clone().into()),
                            ("label".to_owned(), row.label.clone().into()),
                            ("status".to_owned(), row.status.clone().into()),
                            ("action".to_owned(), row.action.clone().into()),
                            ("action_label".to_owned(), row.action_label.clone().into()),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "actions".to_owned(),
            ExposureValue::Array(
                view.actions
                    .iter()
                    .map(|action| {
                        ExposureValue::Map(vec![
                            ("key".to_owned(), action.key.clone().into()),
                            ("label".to_owned(), action.label.clone().into()),
                            ("action".to_owned(), action.action.clone().into()),
                            ("scope".to_owned(), action.scope.clone().into()),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

fn publish_view(view: Option<&ProvisioningView>, exposures: &mut EngineExposures) {
    let mut writer = exposures.writer(SURFACE_NAMESPACE);
    writer.clear_properties();
    let Some(view) = view else {
        writer.visible(false);
        return;
    };
    writer.visible(view.visible);
    writer.property("title", view.title.clone());
    writer.property("description", view.description.clone());
    writer.property("scope_label", view.scope_label.clone());
    writer.property("rows", exposure_map(view).get_map("rows"));
    writer.property("actions", exposure_map(view).get_map("actions"));
}

trait ExposureMapFields {
    fn get_map(&self, name: &str) -> ExposureValue;
}

impl ExposureMapFields for ExposureValue {
    fn get_map(&self, name: &str) -> ExposureValue {
        match self {
            ExposureValue::Map(fields) => fields
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .unwrap_or_else(|| ExposureValue::Array(Vec::new())),
            _ => ExposureValue::Array(Vec::new()),
        }
    }
}

fn advance_active(state: &mut DatasetProvisioningState, registry: &DatasetRegistry) {
    if state.active.is_some() {
        return;
    }
    while let Some(scope) = state.pending.pop() {
        let id = scope_id(&scope);
        if !state.dismissed.contains(&id) && recommended_missing(registry, &scope) {
            state.active = Some(scope);
            return;
        }
    }
}

pub(crate) fn publish_dataset_provisioning_surface(
    registry: Res<DatasetRegistry>,
    workspace: Option<Res<WorkspaceResource>>,
    windows: Query<(), With<Window>>,
    mut state: ResMut<DatasetProvisioningState>,
    mut exposures: ResMut<EngineExposures>,
) {
    let hook_generation = lunco_hooks::generation();
    let dirty = registry.is_changed()
        || workspace.as_ref().is_some_and(|value| value.is_changed())
        || state.is_changed()
        || state.last_hook_generation != hook_generation;
    if !dirty {
        return;
    }
    advance_active(&mut state, &registry);
    let view = state.active.as_ref().and_then(|scope| {
        invoke_policy(&registry, scope, workspace.as_deref(), !windows.is_empty())
            .map_err(|error| warn!("[datasets] {error}"))
            .ok()
    });
    publish_view(view.as_ref(), &mut exposures);
    state.last_hook_generation = hook_generation;
}

#[derive(Event, Clone, Debug)]
pub(crate) struct SetMissingAssetPromptSuppressed {
    pub(crate) root: PathBuf,
    pub(crate) suppressed: bool,
}

#[Command(default)]
pub(crate) struct DismissDatasetProvisioning {
    pub(crate) scope: String,
}

#[Command(default)]
pub(crate) struct SuppressDatasetProvisioning {
    pub(crate) scope: String,
}

fn scope_from_id(state: &DatasetProvisioningState, id: &str) -> Option<DatasetScope> {
    state
        .active
        .iter()
        .chain(state.pending.iter())
        .find(|scope| scope_id(scope) == id)
        .cloned()
}

#[on_command(DismissDatasetProvisioning)]
fn on_dismiss_dataset_provisioning(
    trigger: On<DismissDatasetProvisioning>,
    mut state: ResMut<DatasetProvisioningState>,
) {
    let requested = trigger.event().scope.as_str();
    let id = if requested.is_empty() {
        state.active.as_ref().map(scope_id).unwrap_or_default()
    } else {
        requested.to_owned()
    };
    state.dismissed.insert(id.to_owned());
    if state
        .active
        .as_ref()
        .is_some_and(|scope| scope_id(scope) == id)
    {
        state.active = None;
    }
}

#[on_command(SuppressDatasetProvisioning)]
fn on_suppress_dataset_provisioning(
    trigger: On<SuppressDatasetProvisioning>,
    mut state: ResMut<DatasetProvisioningState>,
    workspace: Option<Res<WorkspaceResource>>,
    mut commands: Commands,
) {
    let id = trigger.event().scope.as_str();
    let Some(scope) = scope_from_id(&state, id) else {
        warn!("[datasets] cannot suppress unknown provisioning scope `{id}`");
        return;
    };
    if let Some(root) = project_root_for_scope(&scope, workspace.as_deref()) {
        commands.trigger(SetMissingAssetPromptSuppressed {
            root,
            suppressed: true,
        });
    }
    state.dismissed.insert(id.to_owned());
    if state
        .active
        .as_ref()
        .is_some_and(|active| scope_id(active) == id)
    {
        state.active = None;
    }
}

register_commands!(
    on_dismiss_dataset_provisioning,
    on_suppress_dataset_provisioning
);

fn project_root_for_scope(
    scope: &DatasetScope,
    workspace: Option<&WorkspaceResource>,
) -> Option<PathBuf> {
    let root = match scope {
        DatasetScope::Twin { root, .. } => Some(root.clone()),
        DatasetScope::Engine => workspace.and_then(|workspace| {
            let id = workspace.active_twin?;
            workspace.twin(id).map(|twin| twin.root.clone())
        }),
    }?;
    workspace
        .filter(|workspace| {
            workspace
                .twins()
                .any(|(_, twin)| twin.root == root && twin.manifest.is_some())
        })
        .map(|_| root)
}

fn project_suppresses_prompt(root: Option<&Path>, workspace: Option<&WorkspaceResource>) -> bool {
    root.and_then(|root| {
        workspace.and_then(|workspace| {
            workspace
                .twins()
                .find(|(_, twin)| twin.root == root)
                .and_then(|(_, twin)| twin.manifest.as_ref())
        })
    })
    .is_some_and(lunco_twin::TwinManifest::suppress_missing_asset_prompt)
}

pub(crate) fn active_project_prompt_setting(
    workspace: Option<&WorkspaceResource>,
) -> Option<(PathBuf, bool)> {
    let workspace = workspace?;
    let id = workspace.active_twin?;
    let twin = workspace.twin(id)?;
    let manifest = twin.manifest.as_ref()?;
    Some((twin.root.clone(), manifest.suppress_missing_asset_prompt()))
}

pub(crate) fn on_set_missing_asset_prompt_suppressed(
    trigger: On<SetMissingAssetPromptSuppressed>,
    mut workspace: ResMut<WorkspaceResource>,
) {
    let event = trigger.event();
    let Some((id, _)) = workspace.twins().find(|(_, twin)| twin.root == event.root) else {
        warn!(
            "[datasets] cannot save missing-asset prompt setting: project `{}` is not open",
            event.root.display()
        );
        return;
    };
    let Some(twin) = workspace.twin_mut(id) else {
        return;
    };
    let Some(manifest) = twin.manifest.as_mut() else {
        warn!(
            "[datasets] cannot save missing-asset prompt setting: `{}` has no twin.toml",
            event.root.display()
        );
        return;
    };
    if manifest.suppress_missing_asset_prompt() == event.suppressed {
        return;
    }
    manifest.downloads = event.suppressed.then_some(lunco_twin::DownloadManifest {
        suppress_missing_prompt: true,
    });
    if let Err(error) = twin.save_manifest() {
        warn!(
            "[datasets] could not save missing-asset prompt setting to `{}`: {error}",
            event.root.display()
        );
    }
}

pub(crate) fn on_dataset_scope_ready(
    trigger: On<DatasetScopeReady>,
    registry: Res<DatasetRegistry>,
    mut state: ResMut<DatasetProvisioningState>,
) {
    let scope = &trigger.event().scope;
    if !same_scope(&state, scope) && recommended_missing(&registry, scope) {
        state.pending.push(scope.clone());
    }
}

pub(crate) fn on_dataset_scope_removed(
    trigger: On<DatasetScopeRemoved>,
    mut state: ResMut<DatasetProvisioningState>,
) {
    let scope = &trigger.event().scope;
    state.pending.retain(|candidate| candidate != scope);
    if state
        .active
        .as_ref()
        .is_some_and(|candidate| candidate == scope)
    {
        state.active = None;
    }
    state.dismissed.remove(&scope_id(scope));
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<DatasetProvisioningState>()
        .init_resource::<EngineExposures>()
        .add_observer(on_dataset_scope_ready)
        .add_observer(on_dataset_scope_removed)
        .add_observer(on_set_missing_asset_prompt_suppressed)
        .add_systems(Update, publish_dataset_provisioning_surface);
    register_all_commands(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_states_are_the_only_requestable_states() {
        assert!(needs_provisioning(&DatasetState::Missing));
        assert!(needs_provisioning(&DatasetState::Cancelled));
        assert!(!needs_provisioning(&DatasetState::Installed));
        assert!(!needs_provisioning(&DatasetState::Downloading {
            bytes_done: 1,
            bytes_total: 2
        }));
    }
}
