//! Policy-owned dataset consent and progress presentation.
//!
//! The dataset registry and downloader expose typed facts and generic commands.
//! Rhai decides whether a scope is visible, which rows/actions exist, and what
//! each mode means. The UI only validates the returned view model and publishes
//! it to the generic HTML/CSS runtime surface.

use std::collections::HashSet;

use bevy::prelude::*;
use lunco_assets_datasets::{
    DatasetRegistry, DatasetScope, DatasetScopeRemoved, DatasetState, SceneDatasetRequirements,
};
use lunco_core::{AppSceneExt, Command, on_command, register_commands};
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
    last_interactive: Option<bool>,
    last_simulation: Option<HookValue>,
    expanded: std::collections::HashMap<String, HashSet<String>>,
    pub(super) settings_groups: Vec<DatasetSettingsGroup>,
}

#[derive(Clone, Debug)]
pub(super) struct ProvisioningRow {
    pub(super) id: String,
    parameter_id: String,
    pub(super) label: String,
    pub(super) status: String,
    pub(super) download_status: String,
    pub(super) preparation_status: String,
    pub(super) detail: String,
    pub(super) source_url: String,
    pub(super) source_files: String,
    details_display: String,
    details_label: String,
    min_height: String,
    stages_display: String,
    pub(super) action_display: String,
    pub(super) action: String,
    pub(super) action_label: String,
    pub(super) scope: String,
    pub(super) ready: bool,
    pub(super) rebuild_action: String,
    pub(super) rebuild_label: String,
}

#[derive(Clone, Debug)]
pub(super) struct DatasetSettingsGroup {
    pub(super) owner: String,
    pub(super) scope: String,
    pub(super) rows: Result<Vec<ProvisioningRow>, String>,
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
    simulation_status: String,
    simulation_detail: String,
    rows: Vec<ProvisioningRow>,
    actions: Vec<ProvisioningAction>,
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

fn required_missing(
    registry: &DatasetRegistry,
    requirements: Option<&SceneDatasetRequirements>,
    scope: &DatasetScope,
) -> bool {
    registry.entries().iter().any(|entry| {
        &entry.scope == scope
            && requirements.is_some_and(|requirements| requirements.ids.contains(&entry.id))
            && needs_provisioning(&entry.state)
    })
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
            "simulation_status",
            "simulation_detail",
            "rows",
            "actions",
        ],
        "assets.provision result",
    )?;
    let visible = bool_field(fields, "visible", "assets.provision result")?;
    let title = string_field(fields, "title", "assets.provision result")?;
    let description = string_field(fields, "description", "assets.provision result")?;
    let scope_label = string_field(fields, "scope_label", "assets.provision result")?;
    let simulation_status = string_field(fields, "simulation_status", "assets.provision result")?;
    let simulation_detail = string_field(fields, "simulation_detail", "assets.provision result")?;
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
                    "download_status",
                    "preparation_status",
                    "detail",
                    "source_url",
                    "source_files",
                    "details_display",
                    "details_label",
                    "min_height",
                    "stages_display",
                    "action_display",
                    "rebuild_action",
                    "rebuild_label",
                    "ready",
                    "action",
                    "action_label",
                    "scope",
                ],
                &context,
            )?;
            Ok(ProvisioningRow {
                id: string_field(fields, "id", &context)?,
                parameter_id: string_field(fields, "parameter_id", &context)?,
                label: string_field(fields, "label", &context)?,
                status: string_field(fields, "status", &context)?,
                download_status: string_field(fields, "download_status", &context)?,
                preparation_status: string_field(fields, "preparation_status", &context)?,
                detail: string_field(fields, "detail", &context)?,
                source_url: string_field(fields, "source_url", &context)?,
                source_files: string_field(fields, "source_files", &context)?,
                details_display: string_field(fields, "details_display", &context)?,
                details_label: string_field(fields, "details_label", &context)?,
                min_height: string_field(fields, "min_height", &context)?,
                stages_display: string_field(fields, "stages_display", &context)?,
                action_display: string_field(fields, "action_display", &context)?,
                ready: bool_field(fields, "ready", &context)?,
                rebuild_action: string_field(fields, "rebuild_action", &context)?,
                rebuild_label: string_field(fields, "rebuild_label", &context)?,
                action: string_field(fields, "action", &context)?,
                action_label: string_field(fields, "action_label", &context)?,
                scope: string_field(fields, "scope", &context)?,
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
        if row.action.trim().is_empty()
            || row.action_label.trim().is_empty()
            || row.scope.trim().is_empty()
        {
            return Err(format!(
                "assets.provision row `{}` requires an action, action label, and scope",
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
        simulation_status,
        simulation_detail,
        rows,
        actions,
    })
}

fn state_value(state: &DatasetState) -> HookValue {
    HookValue::map([
        ("kind", HookValue::str(state.kind())),
        (
            "source",
            match state {
                DatasetState::Downloading { source, .. } => HookValue::str(source),
                _ => HookValue::Unit,
            },
        ),
        (
            "detail",
            match state {
                DatasetState::Failed(error) => HookValue::str(error.clone()),
                DatasetState::Processing { kind } => HookValue::str(kind.clone()),
                _ => HookValue::Unit,
            },
        ),
        (
            "bytes_done",
            match state {
                DatasetState::Downloading { bytes_done, .. } => HookValue::UInt(*bytes_done),
                _ => HookValue::Unit,
            },
        ),
        (
            "bytes_total",
            match state {
                DatasetState::Downloading { bytes_total, .. } => HookValue::UInt(*bytes_total),
                _ => HookValue::Unit,
            },
        ),
    ])
}

fn invoke_policy(
    registry: &DatasetRegistry,
    requirements: Option<&SceneDatasetRequirements>,
    scope: &DatasetScope,
    interactive: bool,
    simulation: HookValue,
    expanded: Option<&HashSet<String>>,
    include_optional: bool,
    group: Option<&str>,
) -> Result<ProvisioningView, String> {
    let datasets = registry
        .entries()
        .iter()
        .filter(|entry| &entry.scope == scope && group.is_none_or(|group| entry.group == group))
        .map(|entry| {
            HookValue::map([
                ("id", HookValue::str(entry.id.clone())),
                ("key", HookValue::str(entry.key.clone())),
                ("name", HookValue::str(entry.name.clone())),
                ("recommended", HookValue::Bool(entry.recommended)),
                (
                    "required_for_scene",
                    HookValue::Bool(requirements.is_some_and(|r| r.ids.contains(&entry.id))),
                ),
                ("processed", HookValue::Bool(entry.spec.process.is_some())),
                ("state", state_value(&entry.state)),
                (
                    "expanded",
                    HookValue::Bool(expanded.is_some_and(|ids| ids.contains(&entry.id))),
                ),
                ("inputs_ready", HookValue::Bool(entry.inputs_ready())),
                (
                    "inputs",
                    HookValue::Array(
                        entry
                            .inputs
                            .iter()
                            .map(|input| {
                                HookValue::map([
                                    ("id", HookValue::str(input.id.clone())),
                                    ("name", HookValue::str(input.name.clone())),
                                    ("url", HookValue::str(input.url.clone())),
                                    ("state", HookValue::str(input.state.kind())),
                                    (
                                        "error",
                                        input
                                            .state
                                            .error()
                                            .map(HookValue::str)
                                            .unwrap_or(HookValue::Unit),
                                    ),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ])
        })
        .collect();
    let facts = HookValue::map([
        ("scope", HookValue::str(scope_kind(scope))),
        ("scope_id", HookValue::str(scope.id())),
        ("owner", HookValue::str(scope.label())),
        ("interactive", HookValue::Bool(interactive)),
        ("datasets", HookValue::Array(datasets)),
        ("simulation", simulation),
        ("include_optional", HookValue::Bool(include_optional)),
    ]);
    match lunco_hooks::invoke_unclassified(lunco_core_session::DATASET_PROVISION_HOOK, &[facts]) {
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
                            (
                                "download_status".to_owned(),
                                row.download_status.clone().into(),
                            ),
                            (
                                "preparation_status".to_owned(),
                                row.preparation_status.clone().into(),
                            ),
                            ("detail".to_owned(), row.detail.clone().into()),
                            ("source_url".to_owned(), row.source_url.clone().into()),
                            ("source_files".to_owned(), row.source_files.clone().into()),
                            (
                                "details_display".to_owned(),
                                row.details_display.clone().into(),
                            ),
                            ("details_label".to_owned(), row.details_label.clone().into()),
                            ("min_height".to_owned(), row.min_height.clone().into()),
                            (
                                "stages_display".to_owned(),
                                row.stages_display.clone().into(),
                            ),
                            (
                                "action_display".to_owned(),
                                row.action_display.clone().into(),
                            ),
                            ("action".to_owned(), row.action.clone().into()),
                            ("action_label".to_owned(), row.action_label.clone().into()),
                            ("scope".to_owned(), row.scope.clone().into()),
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
    exposures.retain_across_scenes(SURFACE_NAMESPACE);
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
    writer.property("simulation_status", view.simulation_status.clone());
    writer.property("simulation_detail", view.simulation_detail.clone());
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

fn advance_active(
    state: &mut DatasetProvisioningState,
    registry: &DatasetRegistry,
    requirements: Option<&SceneDatasetRequirements>,
) {
    if state.active.is_some() {
        return;
    }
    while let Some(scope) = state.pending.pop() {
        let id = scope.id();
        if !state.dismissed.contains(&id) && required_missing(registry, requirements, &scope) {
            state.active = Some(scope);
            return;
        }
    }
}

pub(crate) fn publish_dataset_provisioning_surface(
    registry: Res<DatasetRegistry>,
    requirements: Option<Res<SceneDatasetRequirements>>,
    workspace: Option<Res<WorkspaceResource>>,
    windows: Query<(), With<Window>>,
    readiness: Option<Res<lunco_readiness::ReadinessRegistry>>,
    readiness_state: Option<Res<lunco_readiness::ReadinessState>>,
    progress: Option<Res<lunco_core_runtime::SimulationProgress>>,
    transport: Option<Res<lunco_time::TimeTransport>>,
    faults: Option<Res<lunco_core::RuntimeFaults>>,
    mut state: ResMut<DatasetProvisioningState>,
    mut exposures: ResMut<EngineExposures>,
) {
    let hook_generation = lunco_hooks::generation();
    let interactive = !windows.is_empty();
    let simulation = HookValue::map([
        (
            "pending",
            readiness
                .as_ref()
                .map(|r| {
                    HookValue::Array(
                        r.pending()
                            .map(|p| HookValue::str(p.label.clone()))
                            .collect(),
                    )
                })
                .unwrap_or(HookValue::Unit),
        ),
        (
            "world_hold",
            readiness_state
                .as_ref()
                .map(|s| HookValue::Bool(s.world_hold))
                .unwrap_or(HookValue::Unit),
        ),
        (
            "blockers",
            progress
                .as_ref()
                .map(|p| {
                    HookValue::Array(
                        p.blockers()
                            .map(|b| HookValue::str(b.reason.clone()))
                            .collect(),
                    )
                })
                .unwrap_or(HookValue::Unit),
        ),
        (
            "playing",
            transport
                .as_ref()
                .map(|t| HookValue::Bool(t.is_running()))
                .unwrap_or(HookValue::Unit),
        ),
        (
            "fault",
            faults
                .as_ref()
                .and_then(|f| f.first.as_ref())
                .map(|f| {
                    HookValue::map([
                        ("subject", HookValue::str(f.subject.clone())),
                        ("detail", HookValue::str(f.detail.clone())),
                    ])
                })
                .unwrap_or(HookValue::Unit),
        ),
    ]);
    let requirements_changed = requirements.as_ref().is_some_and(|r| r.is_changed());
    if requirements_changed {
        state.active = None;
        state.pending.clear();
        state.dismissed.clear();
        for scope in registry.entries().iter().map(|entry| &entry.scope) {
            if !same_scope(&state, scope)
                && required_missing(&registry, requirements.as_deref(), scope)
            {
                state.pending.push(scope.clone());
            }
        }
    }
    let dirty = requirements_changed
        || registry.is_changed()
        || workspace.as_ref().is_some_and(|value| value.is_changed())
        || state.is_changed()
        || state.last_hook_generation != hook_generation
        || state.last_interactive != Some(interactive)
        || state.last_simulation.as_ref() != Some(&simulation)
        || !exposures.surfaces.contains_key(SURFACE_NAMESPACE);
    if !dirty {
        return;
    }
    if requirements_changed
        || registry.is_changed()
        || state.last_hook_generation != hook_generation
        || workspace.as_ref().is_some_and(|w| w.is_changed())
    {
        let mut groups = std::collections::BTreeMap::new();
        for entry in registry.entries() {
            let group = match &entry.scope {
                DatasetScope::Engine => Some(entry.group.clone()),
                DatasetScope::Twin { .. } => None,
            };
            let owner = group
                .clone()
                .unwrap_or_else(|| entry.scope.label().to_owned());
            groups.insert((entry.scope.id(), owner), (entry.scope.clone(), group));
        }
        state.settings_groups = groups
            .into_iter()
            .map(|((scope_id, owner), (scope, group))| {
                let rows = invoke_policy(
                    &registry,
                    requirements.as_deref(),
                    &scope,
                    interactive,
                    HookValue::Unit,
                    None,
                    true,
                    group.as_deref(),
                )
                .map(|view| view.rows);
                DatasetSettingsGroup {
                    owner,
                    scope: scope_id,
                    rows,
                }
            })
            .collect();
    }
    advance_active(&mut state, &registry, requirements.as_deref());
    let view = state.active.as_ref().and_then(|scope| {
        invoke_policy(
            &registry,
            requirements.as_deref(),
            scope,
            interactive,
            simulation.clone(),
            state.expanded.get(&scope.id()),
            false,
            None,
        )
        .map_err(|error| warn!("[datasets] {error}"))
        .ok()
    });
    publish_view(view.as_ref(), &mut exposures);
    state.last_hook_generation = hook_generation;
    state.last_interactive = Some(interactive);
    state.last_simulation = Some(simulation);
}

#[Command(default)]
pub(crate) struct DismissDatasetProvisioning {
    pub(crate) scope: String,
}

/// Expand or collapse the current dataset's source details.
#[Command]
pub(crate) struct ToggleDatasetDetails {
    pub(crate) id: String,
}

#[on_command(ToggleDatasetDetails)]
fn toggle_dataset_details(
    trigger: On<ToggleDatasetDetails>,
    registry: Res<DatasetRegistry>,
    mut state: ResMut<DatasetProvisioningState>,
) -> Result<lunco_command_contracts::Ack, String> {
    let entry = registry
        .entry(&trigger.event().id)
        .ok_or_else(|| "Dataset is no longer declared".to_owned())?;
    if !state
        .active
        .as_ref()
        .is_some_and(|scope| scope == &entry.scope)
    {
        return Err("Dataset details belong to another or dismissed scope".into());
    }
    let ids = state.expanded.entry(entry.scope.id()).or_default();
    if !ids.remove(&entry.id) {
        ids.insert(entry.id.clone());
    }
    Ok(lunco_command_contracts::Ack::new(
        lunco_command_contracts::OpId::new(),
    ))
}

#[on_command(DismissDatasetProvisioning)]
fn on_dismiss_dataset_provisioning(
    trigger: On<DismissDatasetProvisioning>,
    mut state: ResMut<DatasetProvisioningState>,
) {
    let requested = trigger.event().scope.as_str();
    let id = if requested.is_empty() {
        state
            .active
            .as_ref()
            .map(DatasetScope::id)
            .unwrap_or_default()
    } else {
        requested.to_owned()
    };
    state.dismissed.insert(id.to_owned());
    if state.active.as_ref().is_some_and(|scope| scope.id() == id) {
        state.active = None;
    }
}

register_commands!(toggle_dataset_details, on_dismiss_dataset_provisioning);

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
    state.dismissed.remove(&scope.id());
    state.expanded.remove(&scope.id());
    state
        .settings_groups
        .retain(|group| group.scope != scope.id());
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<DatasetProvisioningState>()
        .init_scene_resource::<EngineExposures>()
        .add_observer(on_dataset_scope_removed)
        .add_systems(Update, publish_dataset_provisioning_surface);
    register_all_commands(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_lifecycle_dataset_projection_input_ticks_and_new_facts() {
        let mut app = App::new();
        app.init_resource::<DatasetRegistry>()
            .init_resource::<DatasetProvisioningState>()
            .init_scene_resource::<EngineExposures>()
            .add_systems(Update, publish_dataset_provisioning_surface);
        app.update();
        let state_tick = app
            .world()
            .get_resource_ref::<DatasetProvisioningState>()
            .unwrap()
            .last_changed();
        let output_tick = app
            .world()
            .get_resource_ref::<EngineExposures>()
            .unwrap()
            .last_changed();
        app.update();
        assert_eq!(
            app.world()
                .get_resource_ref::<DatasetProvisioningState>()
                .unwrap()
                .last_changed(),
            state_tick,
            "quiet updates preserve publication input ticks"
        );
        assert_eq!(
            app.world()
                .get_resource_ref::<EngineExposures>()
                .unwrap()
                .last_changed(),
            output_tick,
            "quiet updates must not republish the surface"
        );

        app.world_mut()
            .resource_mut::<DatasetRegistry>()
            .set_changed();
        app.update();
        let refreshed = app
            .world()
            .get_resource_ref::<EngineExposures>()
            .unwrap()
            .last_changed();
        assert_ne!(
            refreshed, output_tick,
            "registry changes refresh presentation"
        );
        app.update();
        assert_eq!(
            app.world()
                .get_resource_ref::<EngineExposures>()
                .unwrap()
                .last_changed(),
            refreshed
        );

        lunco_core::run_scene_teardown(app.world_mut());
        assert!(
            !app.world()
                .resource::<EngineExposures>()
                .surfaces
                .contains_key(SURFACE_NAMESPACE)
        );
        app.update();
        assert!(
            app.world()
                .resource::<EngineExposures>()
                .surfaces
                .contains_key(SURFACE_NAMESPACE)
        );
        let republished = app
            .world()
            .get_resource_ref::<EngineExposures>()
            .unwrap()
            .last_changed();
        app.update();
        assert_eq!(
            app.world()
                .get_resource_ref::<EngineExposures>()
                .unwrap()
                .last_changed(),
            republished,
            "scene reset wakes the unchanged source exactly once"
        );

        let window = app.world_mut().spawn(Window::default()).id();
        app.update();
        let interactive_tick = app
            .world()
            .get_resource_ref::<EngineExposures>()
            .unwrap()
            .last_changed();
        assert_ne!(
            interactive_tick, refreshed,
            "interactive availability is a policy input"
        );
        app.world_mut().despawn(window);
        app.update();
        assert_ne!(
            app.world()
                .get_resource_ref::<EngineExposures>()
                .unwrap()
                .last_changed(),
            interactive_tick,
            "losing the final window refreshes presentation"
        );
    }

    #[test]
    fn missing_states_are_the_only_requestable_states() {
        assert!(needs_provisioning(&DatasetState::Missing));
        assert!(needs_provisioning(&DatasetState::Cancelled));
        assert!(!needs_provisioning(&DatasetState::Installed));
        assert!(!needs_provisioning(&DatasetState::Downloading {
            source: "Fixture".to_owned(),
            bytes_done: 1,
            bytes_total: 2
        }));
    }
}
