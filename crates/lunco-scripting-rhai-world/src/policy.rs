//! **Scripted-policy activation** — compile rhai policies into the hook registry.
//!
//! The hook substrate ([`lunco_hooks`]) lets internal decisions be authored in
//! rhai: the convergent **merge** order ([`MERGE_SEAM`]), the **authorization**
//! gate, authored actuation policies, and any application-defined seam such as a
//! generated Modelica synthesizer. A policy is an ordinary projected definition;
//! this module owns activation for both standalone and networked applications.
//!
//! Distribution remains outside this module. The application policy bundle and
//! its startup function are selected by the uniquely marked authored policy
//! manifest in the runtime asset tree; an active Twin may add a separate
//! authored Twin policy manifest and policy set. The registry below is only the
//! derived active cache.

use bevy::prelude::*;
use lunco_doc_bevy::JournalResource;
use lunco_hooks::{HookValue, ScriptHook};
use lunco_twin_journal::MergeStrategy;
use rhai::{Dynamic, Engine, ImmutableString};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// The reserved policy seam that drives the journal's convergent merge order.
pub const MERGE_SEAM: &str = "journal.merge.order";

/// The one application startup seam. Its authored function receives the
/// manifest-resolved policy records and installs them through the private
/// `install_manifest_policy` bootstrap binding.
pub const APPLICATION_STARTUP_HOOK: &str = "application.startup";

/// Generic application asset lifecycle policy for authored asset consumers.
pub const APPLICATION_ASSET_HOOK: &str = "application.asset.lifecycle";

/// The generic lifecycle seam for the active Twin. The application startup
/// policy installs the default implementation; a Twin startup policy may
/// replace it for that Twin's authored lifecycle behavior.
pub const TWIN_LIFECYCLE_HOOK: &str = "twin.lifecycle";

lunco_hooks::declare_hook! {
    id: APPLICATION_STARTUP_HOOK,
    owner: "lunco-scripting",
    description: "Install the application or active Twin's manifest-selected policy functions.",
    signature: [policies: ArrayOfMap],
    output: ArrayOfMap,
    deterministic: false,
    required: false,
    installable: false,
}

lunco_hooks::declare_hook! {
    id: APPLICATION_ASSET_HOOK,
    owner: "lunco-assets-runtime",
    description: "Project a changed application or Twin text asset into authored application UI contributions.",
    signature: [event: String, ctx: Map],
    output: Map,
    deterministic: false,
    required: false,
    installable: true,
}

lunco_hooks::declare_hook! {
    id: TWIN_LIFECYCLE_HOOK,
    owner: "lunco-scripting",
    description: "Choose authored behavior for a Twin lifecycle event.",
    signature: [event: String, ctx: Map],
    output: Map,
    deterministic: false,
    required: false,
    installable: true,
}
/// One scripted policy: a rhai `source` whose `entry` function fills the hook at
/// `seam`. The seam is open so future decisions do not require a Rust enum or
/// branch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PolicyDef {
    /// The hook id this policy registers under.
    pub seam: String,
    /// The rhai entry function name.
    pub entry: String,
    /// The rhai source defining `entry` and its helpers.
    pub source: String,
    /// Whether the hook is deterministic (fresh rhai scope per invoke).
    pub deterministic: bool,
}

/// The result of one runtime policy-set load.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PolicyLoadReport {
    /// Scope whose policies were selected.
    pub scope: String,
    /// Hook ids compiled and installed successfully.
    pub installed: Vec<String>,
    /// Hook ids whose source was present but failed to compile or activate.
    pub failed: Vec<String>,
    /// Required policy failures. The shipped application set keeps this empty;
    /// Twin authors may opt a seam into a strict gate with `required = true`.
    pub required_failures: Vec<String>,
    /// Manifest or storage error that prevented the set from being resolved.
    pub error: Option<String>,
}

/// The last lifecycle hook delivery for the active Twin.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LifecyclePolicyReport {
    /// Lifecycle event delivered to the policy.
    pub event: String,
    /// `ok`, `unavailable`, or `fault`.
    pub status: String,
    /// Typed policy result when the hook returned one.
    pub result: Option<HookValue>,
    /// Structured delivery or return-shape error, if any.
    pub error: Option<String>,
}

/// Ordered command plan returned by an authored Twin loading policy.
#[derive(Clone, Debug)]
struct TwinPolicyCommand {
    twin: lunco_workspace::TwinId,
    command: String,
    params: HookValue,
}

/// Commands waiting for the generic typed command bridge to apply them.
#[derive(Resource, Default)]
pub struct PendingTwinPolicyCommands(Vec<TwinPolicyCommand>);

/// The derived set of active scripted policies on this process.
#[derive(Resource, Default, Clone)]
pub struct ScriptedPolicyRegistry {
    /// The definitions currently active after the application/Twin layers are
    /// resolved. A Twin override shadows the application definition by id.
    pub policies: Vec<PolicyDef>,
    /// Last application/Twin load result, exposed for diagnostics and UI.
    pub status: PolicyLoadReport,
    /// Last active-Twin lifecycle hook result, retained for Rhai/API
    /// inspection instead of being discarded by the dispatcher.
    pub lifecycle: LifecyclePolicyReport,
    application_status: PolicyLoadReport,
    application_policies: Vec<PolicyDef>,
    twin_policies: Vec<PolicyDef>,
    application_hooks: HashMap<String, Arc<lunco_hooks::RegisteredHook>>,
    application_bindings: HashMap<String, lunco_hooks::HookPolicyBinding>,
    twin_scope_ids: HashSet<String>,
    twin_hooks: HashMap<String, Arc<lunco_hooks::RegisteredHook>>,
    twin_bindings: HashMap<String, lunco_hooks::HookPolicyBinding>,
    usd_policies: Vec<PolicyDef>,
    usd_scope_ids: HashSet<String>,
    usd_hooks: HashMap<String, Arc<lunco_hooks::RegisteredHook>>,
    usd_bindings: HashMap<String, lunco_hooks::HookPolicyBinding>,
    active_twin: Option<lunco_workspace::TwinId>,
    /// Revision of the committed effective policy layer.
    ///
    /// This is deliberately separate from `lunco_hooks::generation()`: hook
    /// compilation/replacement is an implementation detail while the source
    /// admission policy needs one stable transaction boundary. Consumers use
    /// this revision to react once to the resulting layer, not once per hook.
    pub(crate) revision: u64,
}

#[derive(Default)]
struct StartupInstallState {
    definitions: Vec<PolicyDef>,
    installed: Vec<String>,
    failed: Vec<String>,
    attempted: HashSet<String>,
    reused: HashSet<String>,
}

fn policy_value(loaded: &lunco_assets_runtime::scripting::LoadedPolicy) -> HookValue {
    HookValue::map([
        ("hook", HookValue::str(loaded.spec.hook.clone())),
        ("entry", HookValue::str(loaded.spec.entry.clone())),
        ("source", HookValue::str(loaded.source.clone())),
        ("policy_file", HookValue::str(loaded.policy_file.clone())),
        ("deterministic", HookValue::Bool(loaded.spec.deterministic)),
        ("required", HookValue::Bool(loaded.spec.required)),
    ])
}

fn startup_operation_result(id: &str, ok: bool, error: Option<&str>) -> Dynamic {
    let mut result = rhai::Map::new();
    result.insert("id".into(), Dynamic::from(id.to_owned()));
    result.insert("ok".into(), Dynamic::from_bool(ok));
    result.insert(
        "status".into(),
        Dynamic::from(if ok { "installed" } else { "failed" }),
    );
    result.insert(
        "error".into(),
        error
            .map(|error| Dynamic::from(error.to_owned()))
            .unwrap_or(Dynamic::UNIT),
    );
    Dynamic::from_map(result)
}

fn cleanup_startup_installations(state: &StartupInstallState, journal: Option<&JournalResource>) {
    for definition in &state.definitions {
        if state.reused.contains(&definition.seam) {
            continue;
        }
        retract_policy(&definition.seam, journal);
    }
}

fn validate_startup_results(
    results: &[HookValue],
    loaded: &[lunco_assets_runtime::scripting::LoadedPolicy],
    state: &StartupInstallState,
) -> Result<(), String> {
    let expected = loaded
        .iter()
        .map(|policy| policy.spec.hook.as_str())
        .collect::<HashSet<_>>();
    let mut returned = HashSet::with_capacity(results.len());
    for result in results {
        let id = result
            .get("id")
            .and_then(HookValue::as_str)
            .ok_or_else(|| "startup policy result has no string id".to_owned())?;
        if !expected.contains(id) {
            return Err(format!(
                "startup policy returned an unknown policy result '{id}'"
            ));
        }
        if !returned.insert(id) {
            return Err(format!(
                "startup policy returned duplicate result for '{id}'"
            ));
        }
        let ok = match result.get("ok") {
            Some(HookValue::Bool(ok)) => *ok,
            _ => {
                return Err(format!(
                    "startup policy result for '{id}' has no boolean ok"
                ));
            }
        };
        let status = result
            .get("status")
            .and_then(HookValue::as_str)
            .ok_or_else(|| format!("startup policy result for '{id}' has no status"))?;
        let status_ok = match status {
            "installed" => true,
            "failed" => false,
            _ => {
                return Err(format!(
                    "startup policy result for '{id}' has unknown status '{status}'"
                ));
            }
        };
        if ok != status_ok {
            return Err(format!(
                "startup policy result for '{id}' disagrees between ok and status"
            ));
        }
        let installed = state
            .installed
            .iter()
            .any(|installed_id| installed_id == id);
        let failed = state.failed.iter().any(|failure| {
            failure
                .strip_prefix(id)
                .is_some_and(|suffix| suffix.starts_with(": "))
        });
        if ok != installed || failed == ok {
            return Err(format!(
                "startup policy result for '{id}' disagrees with installation state"
            ));
        }
    }
    if returned.len() != expected.len() || state.attempted.len() != expected.len() {
        return Err(format!(
            "startup policy processed {} of {} manifest policies",
            state.attempted.len(),
            expected.len()
        ));
    }
    Ok(())
}

fn copy_registered_hook(hook: &Arc<lunco_hooks::RegisteredHook>) -> lunco_hooks::RegisteredHook {
    lunco_hooks::RegisteredHook {
        id: hook.id.clone(),
        backend: hook.backend.clone(),
        deterministic: hook.deterministic,
        hook: Arc::clone(&hook.hook),
    }
}

fn invoke_twin_lifecycle(
    event: &str,
    twin_id: lunco_workspace::TwinId,
    root: &Path,
    report: &PolicyLoadReport,
) -> LifecyclePolicyReport {
    let context = HookValue::map([
        ("twin_id", HookValue::str(twin_id.raw().to_string())),
        ("root", HookValue::str(root.display().to_string())),
        ("scope", HookValue::str(report.scope.clone())),
        (
            "installed",
            HookValue::Array(
                report
                    .installed
                    .iter()
                    .map(|id| HookValue::str(id.clone()))
                    .collect(),
            ),
        ),
        (
            "failed",
            HookValue::Array(
                report
                    .failed
                    .iter()
                    .map(|failure| HookValue::str(failure.clone()))
                    .collect(),
            ),
        ),
    ]);
    invoke_twin_lifecycle_context(event, context)
}

fn invoke_twin_lifecycle_context(event: &str, context: HookValue) -> LifecyclePolicyReport {
    match lunco_hooks::invoke(TWIN_LIFECYCLE_HOOK, &[HookValue::str(event), context]) {
        None => LifecyclePolicyReport {
            event: event.to_owned(),
            status: "unavailable".into(),
            ..Default::default()
        },
        Some(Ok(value @ HookValue::Map(_))) => LifecyclePolicyReport {
            event: event.to_owned(),
            status: "ok".into(),
            result: Some(value),
            ..Default::default()
        },
        Some(Ok(value)) => {
            let error = format!(
                "Twin lifecycle event '{event}' returned unexpected {}",
                value.type_name()
            );
            warn!("[policy] {error}");
            LifecyclePolicyReport {
                event: event.to_owned(),
                status: "fault".into(),
                error: Some(error),
                ..Default::default()
            }
        }
        Some(Err(error)) => {
            warn!("[policy] Twin lifecycle event '{event}' failed: {error}");
            LifecyclePolicyReport {
                event: event.to_owned(),
                status: "fault".into(),
                error: Some(error.to_string()),
                ..Default::default()
            }
        }
    }
}

/// Notify the installed application policy that one JSON asset scope is loading.
pub fn handle_application_json_scope_loading(
    trigger: On<lunco_assets_runtime::JsonAssetScopeLoading>,
    mut commands: Commands,
) {
    let scope = trigger.event();
    let provider = match asset_menu_provider(scope.twin_id, scope.twin_name.as_deref()) {
        Ok(provider) => provider,
        Err(error) => {
            warn!("[application-assets] invalid JSON scope: {error}");
            return;
        }
    };
    apply_application_asset_policy(
        "json_scope_loading",
        &provider,
        scope.twin_id,
        scope.twin_name.clone(),
        scope.asset_root_uri.clone(),
        HookValue::map([("has_json_assets", HookValue::Bool(scope.asset_count != 0))]),
        &mut commands,
    );
}

/// Deliver a complete generic JSON asset snapshot to the installed application
/// policy, replacing that scope's complete menu contribution.
pub fn handle_application_json_scope_changed(
    trigger: On<lunco_assets_runtime::JsonAssetScopeChanged>,
    mut commands: Commands,
) {
    let scope = trigger.event();
    let provider = match asset_menu_provider(scope.twin_id, scope.twin_name.as_deref()) {
        Ok(provider) => provider,
        Err(error) => {
            warn!("[application-assets] invalid JSON scope: {error}");
            return;
        }
    };
    let assets = HookValue::Array(
        scope
            .assets
            .iter()
            .map(|asset| {
                HookValue::map([
                    ("asset_uri", HookValue::str(asset.asset_uri.clone())),
                    (
                        "text",
                        asset
                            .text
                            .as_ref()
                            .map_or(HookValue::Unit, |text| HookValue::str(text.clone())),
                    ),
                    (
                        "error",
                        asset
                            .error
                            .as_ref()
                            .map_or(HookValue::Unit, |error| HookValue::str(error.clone())),
                    ),
                ])
            })
            .collect(),
    );
    apply_application_asset_policy(
        "json_scope_changed",
        &provider,
        scope.twin_id,
        scope.twin_name.clone(),
        scope.asset_root_uri.clone(),
        assets,
        &mut commands,
    );
}

fn asset_menu_provider(
    twin_id: Option<lunco_workspace::TwinId>,
    twin_name: Option<&str>,
) -> Result<String, &'static str> {
    match (twin_id, twin_name) {
        (None, None) => Ok("application:json-assets".to_owned()),
        (Some(_), Some(name)) if !name.is_empty() => Ok(format!("twin:{name}:json-assets")),
        (Some(_), _) => Err("Twin JSON scope is missing its asset authority"),
        (None, Some(_)) => Err("application JSON scope cannot have a Twin authority"),
    }
}

fn apply_application_asset_policy(
    event: &str,
    provider: &str,
    twin_id: Option<lunco_workspace::TwinId>,
    twin_name: Option<String>,
    asset_root_uri: String,
    payload: HookValue,
    commands: &mut Commands,
) {
    let context = HookValue::map([
        ("provider", HookValue::str(provider.to_owned())),
        (
            "scope",
            HookValue::str(if twin_id.is_some() {
                "twin"
            } else {
                "application"
            }),
        ),
        (
            "twin_id",
            twin_id.map_or(HookValue::Unit, |twin| {
                HookValue::str(twin.raw().to_string())
            }),
        ),
        (
            "twin_name",
            twin_name.map_or(HookValue::Unit, |name| HookValue::str(name)),
        ),
        ("asset_root_uri", HookValue::str(asset_root_uri)),
        ("payload", payload),
    ]);
    let result = lunco_hooks::invoke(
        APPLICATION_ASSET_HOOK,
        &[HookValue::str(event.to_owned()), context],
    );
    let menus = match result {
        None => Ok(Vec::new()),
        Some(Ok(value @ HookValue::Map(_))) => parse_script_workbench_menus(&value),
        Some(Ok(value)) => Err(format!(
            "asset lifecycle policy returned {}, expected map",
            value.type_name()
        )),
        Some(Err(error)) => Err(error.to_string()),
    };
    let menus = match menus {
        Ok(menus) => menus,
        Err(error) => {
            warn!("[application-assets] `{provider}` policy failed: {error}");
            Vec::new()
        }
    };
    commands.trigger(
        lunco_scripting_rhai_core::ui_bridge::ScriptUiRequest::WorkbenchMenus {
            provider: provider.to_owned(),
            twin_id: twin_id.map(lunco_workspace::TwinId::raw),
            menus,
        },
    );
}

fn parse_script_workbench_menus(
    value: &HookValue,
) -> Result<Vec<lunco_scripting_rhai_core::ui_bridge::ScriptWorkbenchMenu>, String> {
    use lunco_scripting_rhai_core::ui_bridge::ScriptWorkbenchMenu;

    let menus = match value.get("menus") {
        Some(HookValue::Array(menus)) => menus,
        Some(other) => {
            return Err(format!(
                "`menus` must be an array, got {}",
                other.type_name()
            ))
        }
        None => return Err("result has no `menus` array".into()),
    };
    if menus.len() > 32 {
        return Err("one policy result may contribute at most 32 top-level menus".into());
    }
    let mut labels = HashSet::new();
    menus
        .iter()
        .enumerate()
        .map(|(index, menu)| {
            let fields = hook_map(menu, &format!("menus[{index}]"))?;
            let label = hook_string(fields, "label", &format!("menus[{index}]"))?;
            if label.trim().is_empty() {
                return Err(format!("menus[{index}].label must not be empty"));
            }
            if !labels.insert(label.clone()) {
                return Err(format!("duplicate top-level menu label `{label}`"));
            }
            let entries = match hook_field(fields, "items") {
                Some(HookValue::Array(items)) => items,
                Some(other) => {
                    return Err(format!(
                        "menus[{index}].items must be an array, got {}",
                        other.type_name()
                    ));
                }
                None => return Err(format!("menus[{index}] has no `items` array")),
            };
            let mut count = 0;
            let items = entries
                .iter()
                .enumerate()
                .map(|(entry_index, item)| {
                    parse_script_workbench_item(
                        item,
                        &format!("menus[{index}].items[{entry_index}]"),
                        0,
                        &mut count,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ScriptWorkbenchMenu { label, items })
        })
        .collect()
}

fn parse_script_workbench_item(
    value: &HookValue,
    path: &str,
    depth: usize,
    count: &mut usize,
) -> Result<lunco_scripting_rhai_core::ui_bridge::ScriptWorkbenchMenuItem, String> {
    use lunco_scripting_rhai_core::ui_bridge::{
        ScriptWorkbenchMenuAction, ScriptWorkbenchMenuItem,
    };

    if depth > 8 {
        return Err(format!("{path} exceeds the maximum menu depth of 8"));
    }
    *count += 1;
    if *count > 512 {
        return Err("one policy result may contribute at most 512 menu items".into());
    }
    let fields = hook_map(value, path)?;
    let label = hook_string(fields, "label", path)?;
    if label.trim().is_empty() {
        return Err(format!("{path}.label must not be empty"));
    }
    let tooltip = match hook_field(fields, "tooltip") {
        None | Some(HookValue::Unit) => None,
        Some(HookValue::Str(value)) => Some(value.clone()),
        Some(other) => {
            return Err(format!(
                "{path}.tooltip must be a string, got {}",
                other.type_name()
            ))
        }
    };
    let enabled = match hook_field(fields, "enabled") {
        None => true,
        Some(HookValue::Bool(enabled)) => *enabled,
        Some(other) => {
            return Err(format!(
                "{path}.enabled must be boolean, got {}",
                other.type_name()
            ))
        }
    };
    let children = match hook_field(fields, "children") {
        None => Vec::new(),
        Some(HookValue::Array(children)) => children
            .iter()
            .enumerate()
            .map(|(index, child)| {
                parse_script_workbench_item(
                    child,
                    &format!("{path}.children[{index}]"),
                    depth + 1,
                    count,
                )
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(other) => {
            return Err(format!(
                "{path}.children must be an array, got {}",
                other.type_name()
            ));
        }
    };
    let tool = hook_field(fields, "tool").and_then(HookValue::as_str);
    let hook = hook_field(fields, "hook").and_then(HookValue::as_str);
    let action = match (tool, hook) {
        (Some(tool), Some(hook)) if children.is_empty() => {
            if tool.trim().is_empty() || hook.trim().is_empty() {
                return Err(format!("{path} tool and hook must not be empty"));
            }
            let args = match hook_field(fields, "args") {
                None => lunco_telemetry_core::TelemetryValue::Map(BTreeMap::new()),
                Some(HookValue::Map(args)) => {
                    let mut result = BTreeMap::new();
                    for (key, value) in args {
                        result.insert(
                            key.clone(),
                            hook_to_telemetry(value, &format!("{path}.args.{key}"))?,
                        );
                    }
                    lunco_telemetry_core::TelemetryValue::Map(result)
                }
                Some(other) => {
                    return Err(format!(
                        "{path}.args must be a map, got {}",
                        other.type_name()
                    ))
                }
            };
            Some(ScriptWorkbenchMenuAction {
                tool: tool.to_owned(),
                hook: hook.to_owned(),
                args,
            })
        }
        (None, None) if !children.is_empty() => None,
        (None, None) if !enabled => None,
        (Some(_), Some(_)) => return Err(format!("{path} cannot combine an action with children")),
        (None, None) => return Err(format!("{path} needs an action or children")),
        _ => return Err(format!("{path} must provide both `tool` and `hook`")),
    };
    Ok(ScriptWorkbenchMenuItem {
        label,
        tooltip,
        enabled,
        action,
        children,
    })
}

fn hook_map<'a>(value: &'a HookValue, path: &str) -> Result<&'a [(String, HookValue)], String> {
    match value {
        HookValue::Map(fields) => Ok(fields),
        other => Err(format!("{path} must be a map, got {}", other.type_name())),
    }
}

fn hook_field<'a>(fields: &'a [(String, HookValue)], key: &str) -> Option<&'a HookValue> {
    fields
        .iter()
        .find_map(|(name, value)| (name == key).then_some(value))
}

fn hook_string(fields: &[(String, HookValue)], key: &str, path: &str) -> Result<String, String> {
    hook_field(fields, key)
        .and_then(HookValue::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{path}.{key} must be a string"))
}

fn hook_to_telemetry(
    value: &HookValue,
    path: &str,
) -> Result<lunco_telemetry_core::TelemetryValue, String> {
    use lunco_telemetry_core::TelemetryValue;
    Ok(match value {
        HookValue::Int(value) => TelemetryValue::I64(*value),
        HookValue::Float(value) => TelemetryValue::F64(*value),
        HookValue::Bool(value) => TelemetryValue::Bool(*value),
        HookValue::Str(value) => TelemetryValue::String(value.clone()),
        HookValue::Array(values) => TelemetryValue::Array(
            values
                .iter()
                .enumerate()
                .map(|(index, value)| hook_to_telemetry(value, &format!("{path}[{index}]")))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        HookValue::Map(fields) => {
            let mut result = BTreeMap::new();
            for (key, value) in fields {
                result.insert(
                    key.clone(),
                    hook_to_telemetry(value, &format!("{path}.{key}"))?,
                );
            }
            TelemetryValue::Map(result)
        }
        HookValue::Unit => return Err(format!("{path} cannot contain JSON null in a menu action")),
        HookValue::Bytes(_) => return Err(format!("{path} cannot contain bytes in a menu action")),
    })
}

/// Deliver the mounted Twin's typed manifest and file index to Rhai. The
/// policy selects domain loaders and returns an ordered list of typed command
/// requests; this Rust boundary only validates that generic action shape.
pub fn plan_twin_asset_loading(
    trigger: On<lunco_assets_runtime::TwinAssetMounted>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut registry: ResMut<ScriptedPolicyRegistry>,
    mut pending: ResMut<PendingTwinPolicyCommands>,
) {
    let twin_id = trigger.event().twin;
    let Some(workspace) = workspace.as_deref() else {
        return;
    };
    let Some(twin) = workspace.twin(twin_id) else {
        return;
    };

    let manifest = match twin.manifest.as_ref() {
        Some(manifest) => match lunco_api_core::api_value_from_serializable(manifest) {
            Ok(value) => value,
            Err(error) => {
                registry.lifecycle = LifecyclePolicyReport {
                    event: "assets_mounted".into(),
                    status: "fault".into(),
                    error: Some(format!("cannot expose Twin manifest to Rhai: {error}")),
                    ..Default::default()
                };
                return;
            }
        },
        None => HookValue::Unit,
    };
    let files = HookValue::Array(
        twin.files()
            .iter()
            .map(|entry| HookValue::str(lunco_assets_path::slashed(&entry.relative_path)))
            .collect(),
    );
    let context = HookValue::map([
        ("twin_id", HookValue::Int(twin_id.raw() as i64)),
        ("name", HookValue::str(trigger.event().name.clone())),
        ("root", HookValue::str(twin.root.display().to_string())),
        (
            "active",
            HookValue::Bool(workspace.active_twin == Some(twin_id)),
        ),
        ("manifest", manifest),
        ("files", files),
    ]);
    let report = invoke_twin_lifecycle_context("assets_mounted", context);
    let Some(result) = report.result.as_ref() else {
        registry.lifecycle = report;
        return;
    };

    let actions = match result.get("actions") {
        None | Some(HookValue::Unit) => Vec::new(),
        Some(HookValue::Array(actions)) => actions.clone(),
        Some(value) => {
            registry.lifecycle = LifecyclePolicyReport {
                status: "fault".into(),
                error: Some(format!(
                    "Twin loading policy actions must be an array, got {}",
                    value.type_name()
                )),
                ..report
            };
            return;
        }
    };

    let mut parsed = Vec::with_capacity(actions.len());
    for (index, action) in actions.into_iter().enumerate() {
        let HookValue::Map(fields) = action else {
            registry.lifecycle = LifecyclePolicyReport {
                status: "fault".into(),
                error: Some(format!("Twin loading policy action {index} must be a map")),
                ..report
            };
            return;
        };
        let action = HookValue::Map(fields);
        let Some(command) = action.get("command").and_then(HookValue::as_str) else {
            registry.lifecycle = LifecyclePolicyReport {
                status: "fault".into(),
                error: Some(format!(
                    "Twin loading policy action {index} has no string command"
                )),
                ..report
            };
            return;
        };
        let params = match action.get("params") {
            Some(HookValue::Map(_)) => action.get("params").cloned().unwrap_or_default(),
            Some(HookValue::Unit) | None => HookValue::Map(Vec::new()),
            Some(value) => {
                registry.lifecycle = LifecyclePolicyReport {
                    status: "fault".into(),
                    error: Some(format!(
                        "Twin loading policy action {index} params must be a map, got {}",
                        value.type_name()
                    )),
                    ..report
                };
                return;
            }
        };
        parsed.push(TwinPolicyCommand {
            twin: twin_id,
            command: command.to_owned(),
            params,
        });
    }
    if let Some(message) = result
        .get("info")
        .and_then(HookValue::as_str)
        .filter(|message| !message.is_empty())
    {
        info!("[twin-loading] {message}");
    }
    pending.0.extend(parsed);
    registry.lifecycle = report;
}

/// Apply Rhai's ordered loader plan through the same reflected typed command
/// bridge used by scripts. The executor knows no Twin fields or domain loaders.
pub fn apply_twin_policy_commands(world: &mut World) {
    let queued = std::mem::take(&mut world.resource_mut::<PendingTwinPolicyCommands>().0);
    if queued.is_empty() {
        return;
    }
    for action in queued {
        let current = world
            .get_resource::<lunco_workspace::WorkspaceResource>()
            .is_some_and(|workspace| workspace.active_twin == Some(action.twin));
        if !current {
            continue;
        }
        let _scope = lunco_scripting_bridge_core::WorldScope::enter(world);
        let result = lunco_scripting_bridge_core::cmd_value(&action.command, action.params);
        match result.get("ok").and_then(HookValue::as_bool) {
            Some(true) => {}
            _ => {
                let detail = result
                    .get("error")
                    .and_then(HookValue::as_str)
                    .unwrap_or("typed command was rejected");
                warn!(
                    "[twin-loading] policy command `{}` failed: {detail}",
                    action.command
                );
            }
        }
    }
}

fn all_policy_ids(registry: &ScriptedPolicyRegistry) -> HashSet<String> {
    registry
        .policies
        .iter()
        .chain(&registry.application_policies)
        .chain(&registry.twin_policies)
        .chain(&registry.usd_policies)
        .map(|definition| definition.seam.clone())
        .chain(registry.twin_scope_ids.iter().cloned())
        .chain(registry.usd_scope_ids.iter().cloned())
        .chain(registry.application_hooks.keys().cloned())
        .chain(registry.twin_hooks.keys().cloned())
        .chain(registry.usd_hooks.keys().cloned())
        .collect()
}

fn layer_hook(
    registry: &ScriptedPolicyRegistry,
    id: &str,
) -> Option<(
    Arc<lunco_hooks::RegisteredHook>,
    Option<lunco_hooks::HookPolicyBinding>,
)> {
    if registry.usd_scope_ids.contains(id) {
        return registry
            .usd_hooks
            .get(id)
            .map(|hook| (Arc::clone(hook), registry.usd_bindings.get(id).cloned()));
    }
    if registry.twin_scope_ids.contains(id) {
        return registry
            .twin_hooks
            .get(id)
            .map(|hook| (Arc::clone(hook), registry.twin_bindings.get(id).cloned()));
    }
    registry.application_hooks.get(id).map(|hook| {
        (
            Arc::clone(hook),
            registry.application_bindings.get(id).cloned(),
        )
    })
}

fn effective_policy_definitions(registry: &ScriptedPolicyRegistry) -> BTreeMap<String, PolicyDef> {
    let mut active = BTreeMap::new();
    for definition in &registry.application_policies {
        if !registry.twin_scope_ids.contains(&definition.seam)
            && !registry.usd_scope_ids.contains(&definition.seam)
        {
            active.insert(definition.seam.clone(), definition.clone());
        }
    }
    for definition in &registry.twin_policies {
        if !registry.usd_scope_ids.contains(&definition.seam) {
            active.insert(definition.seam.clone(), definition.clone());
        }
    }
    for definition in &registry.usd_policies {
        active.insert(definition.seam.clone(), definition.clone());
    }
    active
}

fn rebuild_active_policy_registry(
    registry: &mut ScriptedPolicyRegistry,
    journal: Option<&JournalResource>,
) {
    let ids = all_policy_ids(registry);
    let active = effective_policy_definitions(registry);
    registry.policies = active.values().cloned().collect();

    // Reconcile only the seams whose effective implementation changed. A
    // Twin load commonly shadows the application layer with the same policy
    // set; tearing down and rebuilding every hook here used to create one
    // asset-admission wave per intermediate registry generation.
    for id in ids {
        let Some(_definition) = active.get(&id) else {
            lunco_hooks::unregister(&id);
            lunco_hooks::unbind_policy(&id);
            continue;
        };
        let desired = layer_hook(registry, &id);
        let current = lunco_hooks::get(&id);
        let same_registration = desired
            .as_ref()
            .zip(current.as_ref())
            .is_some_and(|(desired, current)| Arc::ptr_eq(&desired.0, current));
        if !same_registration {
            if let Some((hook, _)) = desired.as_ref() {
                lunco_hooks::register(copy_registered_hook(hook));
            }
        }
        if let Some((_, binding)) = desired {
            if let Some(binding) = binding {
                lunco_hooks::bind_policy(id, binding);
            } else {
                lunco_hooks::unbind_policy(&id);
            }
        } else {
            // An installed definition without its layer registration is an
            // invalid internal state; do not retain a lower-layer hook under
            // a higher-layer policy name.
            lunco_hooks::unregister(&id);
            lunco_hooks::unbind_policy(&id);
        }
    }

    if let Some(journal) = journal {
        if lunco_hooks::get(MERGE_SEAM).is_some() {
            journal.with_write(|journal| {
                journal.set_merge_strategy(MergeStrategy::Scripted(MERGE_SEAM.into()))
            });
        } else {
            use_default_merge_policy(journal);
        }
    }
}

fn wind_down_twin_policies(
    registry: &mut ScriptedPolicyRegistry,
    journal: Option<&JournalResource>,
) {
    registry.twin_policies.clear();
    registry.twin_scope_ids.clear();
    registry.twin_hooks.clear();
    registry.twin_bindings.clear();
    // USD policy prims are scene-owned. They must not survive the Twin that
    // authored them, even for the short interval before the next scene is
    // projected.
    registry.usd_policies.clear();
    registry.usd_scope_ids.clear();
    registry.usd_hooks.clear();
    registry.usd_bindings.clear();
    rebuild_active_policy_registry(registry, journal);
}

fn run_startup_policy(
    startup: lunco_assets_runtime::scripting::LoadedStartup,
    loaded: &[lunco_assets_runtime::scripting::LoadedPolicy],
    journal: Option<&JournalResource>,
    reusable: &HashMap<String, PolicyDef>,
) -> Result<StartupInstallState, String> {
    let state = Arc::new(Mutex::new(StartupInstallState::default()));
    let callback_state = Arc::clone(&state);
    let callback_journal = journal.cloned();
    let reusable = reusable.clone();
    let startup_hook = lunco_hooks_rhai::RhaiHook::compile_with(
        &startup.source,
        startup.spec.entry.clone(),
        move |engine: &mut Engine| {
            engine.register_fn(
                "install_manifest_policy",
                move |id: ImmutableString,
                      entry: ImmutableString,
                      source: ImmutableString,
                      policy_file: ImmutableString,
                      deterministic: bool,
                      _required: bool|
                      -> Dynamic {
                    let definition = PolicyDef {
                        seam: id.to_string(),
                        entry: entry.to_string(),
                        source: source.to_string(),
                        deterministic,
                    };
                    let mut state = callback_state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    state.attempted.insert(definition.seam.clone());
                    if reusable.get(&definition.seam) == Some(&definition)
                        && lunco_hooks::get(&definition.seam).is_some()
                    {
                        state.installed.push(definition.seam.clone());
                        state.reused.insert(definition.seam.clone());
                        state.definitions.push(definition);
                        return startup_operation_result(id.as_str(), true, None);
                    }
                    let result = apply_policy(&definition, callback_journal.as_ref());
                    match result {
                        Ok(()) => {
                            lunco_hooks::bind_policy(
                                definition.seam.clone(),
                                lunco_hooks::HookPolicyBinding {
                                    policy_file: policy_file.to_string(),
                                    policy_entry: definition.entry.clone(),
                                },
                            );
                            state.installed.push(definition.seam.clone());
                            state.definitions.push(definition);
                            startup_operation_result(id.as_str(), true, None)
                        }
                        Err(error) => {
                            state.failed.push(format!("{}: {error}", definition.seam));
                            startup_operation_result(id.as_str(), false, Some(error.as_str()))
                        }
                    }
                },
            );
        },
    )?;
    let policies = HookValue::Array(loaded.iter().map(policy_value).collect());
    let result = startup_hook
        .invoke(&[policies])
        .map_err(|error| error.to_string());
    drop(startup_hook);
    let state = Arc::try_unwrap(state)
        .map_err(|_| "application startup policy retained an active callback".to_owned())?
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let output = match result {
        Ok(output) => output,
        Err(error) => {
            cleanup_startup_installations(&state, journal);
            return Err(format!(
                "startup policy '{}' failed ({}): {error}",
                startup.policy_file, startup.spec.entry
            ));
        }
    };
    let HookValue::Array(results) = output else {
        cleanup_startup_installations(&state, journal);
        return Err(format!(
            "startup policy '{}' returned no policy result array",
            startup.policy_file
        ));
    };
    if !results
        .iter()
        .all(|result| matches!(result, HookValue::Map(_)))
    {
        cleanup_startup_installations(&state, journal);
        return Err(format!(
            "startup policy '{}' returned a non-map policy result",
            startup.policy_file
        ));
    }
    if let Err(error) = validate_startup_results(&results, loaded, &state) {
        cleanup_startup_installations(&state, journal);
        return Err(format!("startup policy '{}': {error}", startup.policy_file));
    }
    Ok(state)
}

/// Compile and register a policy, activating the journal merge strategy when
/// the reserved merge seam is used.
pub fn apply_policy(def: &PolicyDef, journal: Option<&JournalResource>) -> Result<(), String> {
    let deterministic = def.seam == MERGE_SEAM || def.deterministic;
    let Some(contract) = lunco_hooks::descriptor(&def.seam) else {
        return Err(format!(
            "hook '{}' has no owner declaration; policy hooks must be declared with declare_hook!",
            def.seam
        ));
    };
    if !contract.installable {
        return Err(format!("hook '{}' is not installable at runtime", def.seam));
    }
    if contract.deterministic && !deterministic {
        return Err(format!(
            "hook '{}' requires a deterministic policy manifest entry",
            def.seam
        ));
    }
    if def.seam == MERGE_SEAM {
        lunco_hooks_rhai::register_rhai_hook(&def.seam, &def.entry, &def.source, true)?;
        if let Some(journal) = journal {
            journal.with_write(|journal| {
                journal.set_merge_strategy(MergeStrategy::Scripted(def.seam.clone()))
            });
        }
        return Ok(());
    }
    lunco_hooks_rhai::register_rhai_hook(&def.seam, &def.entry, &def.source, deterministic)
        .map(|_| ())
}

/// Deactivate a policy whose definition vanished and restore the default
/// journal order when it owned the merge seam.
pub fn retract_policy(seam: &str, journal: Option<&JournalResource>) {
    lunco_hooks::unregister(seam);
    lunco_hooks::unbind_policy(seam);
    if seam == MERGE_SEAM {
        if let Some(journal) = journal {
            use_default_merge_policy(journal);
        }
    }
}

/// Project the desired USD-authored policy layer into the live hook registry.
/// USD policy prims have the highest precedence over application and Twin
/// manifests; a failed USD override blocks lower layers until the prim is
/// removed. Compiled lower layers are retained and restored without a rebuild.
pub fn project_policies(
    desired: Vec<PolicyDef>,
    registry: &mut ScriptedPolicyRegistry,
    journal: Option<&JournalResource>,
) -> Vec<String> {
    let previous = registry.policies.clone();
    let desired = desired
        .into_iter()
        .fold(BTreeMap::new(), |mut policies, policy| {
            policies.insert(policy.seam.clone(), policy);
            policies
        })
        .into_values()
        .collect::<Vec<_>>();
    let old_ids = all_policy_ids(registry);
    for id in old_ids {
        lunco_hooks::unregister(&id);
        lunco_hooks::unbind_policy(&id);
    }
    registry.usd_policies.clear();
    registry.usd_hooks.clear();
    registry.usd_bindings.clear();
    registry.usd_scope_ids = desired.iter().map(|policy| policy.seam.clone()).collect();
    rebuild_active_policy_registry(registry, journal);

    let mut failures = Vec::new();
    let mut installed = Vec::new();
    for def in &desired {
        if let Err(error) = apply_policy(def, journal) {
            warn!("[policy] failed to project seam '{}': {error}", def.seam);
            lunco_hooks::unregister(&def.seam);
            failures.push(format!("{}: {error}", def.seam));
        } else {
            installed.push(def.clone());
        }
    }
    registry.usd_policies = installed;
    registry.usd_hooks = registry
        .usd_policies
        .iter()
        .filter_map(|policy| lunco_hooks::get(&policy.seam).map(|hook| (policy.seam.clone(), hook)))
        .collect();
    rebuild_active_policy_registry(registry, journal);
    if registry.policies != previous {
        registry.revision = registry.revision.wrapping_add(1);
    }
    failures
}

fn coalesce_loaded_policies(
    loaded: impl IntoIterator<Item = lunco_assets_runtime::scripting::LoadedPolicy>,
) -> Vec<lunco_assets_runtime::scripting::LoadedPolicy> {
    loaded
        .into_iter()
        .fold(BTreeMap::new(), |mut policies, policy| {
            policies.insert(policy.spec.hook.clone(), policy);
            policies
        })
        .into_values()
        .collect()
}

fn clear_active_policies(registry: &mut ScriptedPolicyRegistry, journal: Option<&JournalResource>) {
    for id in all_policy_ids(registry) {
        lunco_hooks::unregister(&id);
        lunco_hooks::unbind_policy(&id);
    }
    registry.application_policies.clear();
    registry.application_hooks.clear();
    registry.application_bindings.clear();
    registry.twin_policies.clear();
    registry.twin_hooks.clear();
    registry.twin_bindings.clear();
    registry.twin_scope_ids.clear();
    registry.usd_policies.clear();
    registry.usd_hooks.clear();
    registry.usd_bindings.clear();
    registry.usd_scope_ids.clear();
    registry.policies.clear();
    registry.active_twin = None;
    registry.application_status = PolicyLoadReport::default();
    rebuild_active_policy_registry(registry, journal);
}

fn report_for_application_policies(
    scope: impl Into<String>,
    application: lunco_assets_runtime::scripting::LoadedPolicyBundle,
    registry: &mut ScriptedPolicyRegistry,
    journal: Option<&JournalResource>,
) -> PolicyLoadReport {
    let scope = scope.into();
    clear_active_policies(registry, journal);
    let Some(application_startup) = application.startup else {
        let report = PolicyLoadReport {
            scope,
            error: Some("application policy set has no startup entry".into()),
            ..Default::default()
        };
        registry.application_status = report.clone();
        registry.status = report.clone();
        return report;
    };
    let application_loaded = application.policies;
    let required = application_loaded
        .iter()
        .filter(|policy| policy.spec.required)
        .map(|policy| policy.spec.hook.clone())
        .collect::<HashSet<_>>();
    let application_run = match run_startup_policy(
        application_startup,
        &application_loaded,
        journal,
        &HashMap::new(),
    ) {
        Ok(run) => run,
        Err(error) => {
            let report = PolicyLoadReport {
                scope,
                error: Some(error),
                ..Default::default()
            };
            registry.application_status = report.clone();
            registry.status = report.clone();
            return report;
        }
    };
    let StartupInstallState {
        definitions,
        installed,
        failed,
        ..
    } = application_run;
    registry.application_policies = definitions;
    registry.application_hooks = installed
        .iter()
        .filter_map(|id| lunco_hooks::get(id).map(|hook| (id.clone(), hook)))
        .collect();
    registry.application_bindings = application_loaded
        .iter()
        .filter(|policy| installed.contains(&policy.spec.hook))
        .map(|policy| {
            (
                policy.spec.hook.clone(),
                lunco_hooks::HookPolicyBinding {
                    policy_file: policy.policy_file.clone(),
                    policy_entry: policy.spec.entry.clone(),
                },
            )
        })
        .collect();
    registry.twin_policies.clear();
    registry.twin_hooks.clear();
    registry.twin_bindings.clear();
    registry.twin_scope_ids.clear();
    registry.usd_policies.clear();
    registry.usd_hooks.clear();
    registry.usd_bindings.clear();
    registry.usd_scope_ids.clear();
    rebuild_active_policy_registry(registry, journal);

    let required_failures = failed
        .iter()
        .filter(|failure| {
            failure
                .split_once(':')
                .is_some_and(|(id, _)| required.contains(id))
        })
        .cloned()
        .collect();
    let report = PolicyLoadReport {
        scope,
        installed,
        failed,
        required_failures,
        error: None,
    };
    registry.application_status = report.clone();
    registry.status = report.clone();
    report
}

fn report_for_twin_policies(
    scope: impl Into<String>,
    twin: Option<lunco_assets_runtime::scripting::LoadedPolicyBundle>,
    registry: &mut ScriptedPolicyRegistry,
    journal: Option<&JournalResource>,
) -> PolicyLoadReport {
    let scope = scope.into();
    wind_down_twin_policies(registry, journal);
    let Some(twin) = twin else {
        let report = PolicyLoadReport {
            scope,
            installed: registry
                .application_policies
                .iter()
                .map(|definition| definition.seam.clone())
                .collect(),
            ..Default::default()
        };
        registry.status = report.clone();
        return report;
    };
    let twin_ids = twin
        .policies
        .iter()
        .map(|policy| policy.spec.hook.clone())
        .collect::<HashSet<_>>();
    let required = twin
        .policies
        .iter()
        .filter(|policy| policy.spec.required)
        .map(|policy| policy.spec.hook.clone())
        .collect::<HashSet<_>>();
    let Some(startup) = twin.startup else {
        registry.twin_scope_ids = twin_ids;
        rebuild_active_policy_registry(registry, journal);
        let report = PolicyLoadReport {
            scope,
            error: Some("Twin policy set has policies but no startup entry".into()),
            ..Default::default()
        };
        registry.status = report.clone();
        return report;
    };
    let reusable = registry
        .policies
        .iter()
        .map(|definition| (definition.seam.clone(), definition.clone()))
        .collect::<HashMap<_, _>>();
    let run = run_startup_policy(startup, &twin.policies, journal, &reusable);
    let (installed, failed, error, definitions) = match run {
        Ok(run) => {
            registry.twin_hooks = run
                .installed
                .iter()
                .filter_map(|id| lunco_hooks::get(id).map(|hook| (id.clone(), hook)))
                .collect();
            registry.twin_bindings = twin
                .policies
                .iter()
                .filter(|policy| run.installed.contains(&policy.spec.hook))
                .map(|policy| {
                    (
                        policy.spec.hook.clone(),
                        lunco_hooks::HookPolicyBinding {
                            policy_file: policy.policy_file.clone(),
                            policy_entry: policy.spec.entry.clone(),
                        },
                    )
                })
                .collect();
            (run.installed, run.failed, None, run.definitions)
        }
        Err(error) => {
            registry.twin_hooks.clear();
            registry.twin_bindings.clear();
            (Vec::new(), Vec::new(), Some(error), Vec::new())
        }
    };
    registry.twin_scope_ids = twin_ids;
    registry.twin_policies = definitions;
    rebuild_active_policy_registry(registry, journal);
    let required_failures = failed
        .iter()
        .filter(|failure| {
            failure
                .split_once(':')
                .is_some_and(|(id, _)| required.contains(id))
        })
        .cloned()
        .collect();
    let report = PolicyLoadReport {
        scope,
        installed,
        failed,
        required_failures,
        error,
    };
    registry.status = report.clone();
    report
}

fn report_load_error(
    scope: impl Into<String>,
    error: String,
    registry: &mut ScriptedPolicyRegistry,
    journal: Option<&JournalResource>,
) -> PolicyLoadReport {
    let scope = scope.into();
    if scope == "application" {
        clear_active_policies(registry, journal);
    } else {
        wind_down_twin_policies(registry, journal);
    }
    let report = PolicyLoadReport {
        scope,
        error: Some(error),
        ..Default::default()
    };
    if report.scope == "application" {
        registry.application_status = report.clone();
    }
    registry.status = report.clone();
    report
}

fn log_report(report: &PolicyLoadReport) {
    if let Some(error) = &report.error {
        error!("[policy] {} could not be loaded: {error}", report.scope);
    }
    for failure in &report.failed {
        warn!("[policy] {} unavailable: {failure}", report.scope);
    }
    if !report.installed.is_empty() {
        info!(
            "[policy] {} installed {} {}",
            report.scope,
            report.installed.len(),
            if report.installed.len() == 1 {
                "policy"
            } else {
                "policies"
            }
        );
    }
}

/// Load application policy bindings at simulation startup.
///
/// The manifest and source files are authored assets. A malformed or missing
/// optional policy is retained in the diagnostic report and leaves the seam
/// unimplemented; it does not panic and does not retain an older implementation.
pub fn load_application_policies(
    registry: &mut ScriptedPolicyRegistry,
    journal: Option<&JournalResource>,
) -> PolicyLoadReport {
    let previous = registry.policies.clone();
    let bundle = match lunco_assets_runtime::scripting::active_policy_bundle() {
        Ok(bundle) => bundle,
        Err(error) => {
            let report = report_load_error("application", error, registry, journal);
            if registry.policies != previous {
                registry.revision = registry.revision.wrapping_add(1);
            }
            return report;
        }
    };
    let report = report_for_application_policies("application", bundle, registry, journal);
    if registry.policies != previous {
        registry.revision = registry.revision.wrapping_add(1);
    }
    report
}

/// Load only the active Twin's optional policy layer.
pub fn load_twin_policies(
    root: &Path,
    registry: &mut ScriptedPolicyRegistry,
    journal: Option<&JournalResource>,
) -> PolicyLoadReport {
    let previous = registry.policies.clone();
    let twin = match lunco_assets_runtime::scripting::twin_policy_set(root) {
        Ok(twin) => twin,
        Err(error) => {
            let report = report_load_error("Twin", error, registry, journal);
            if registry.policies != previous {
                registry.revision = registry.revision.wrapping_add(1);
            }
            return report;
        }
    };
    let twin = twin.map(|mut twin| {
        twin.policies = coalesce_loaded_policies(twin.policies);
        twin
    });
    let report =
        report_for_twin_policies(format!("Twin {}", root.display()), twin, registry, journal);
    if registry.policies != previous {
        registry.revision = registry.revision.wrapping_add(1);
    }
    report
}

/// Startup system for the application policy set.
pub fn load_application_policies_on_startup(
    mut registry: ResMut<ScriptedPolicyRegistry>,
    journal: Option<Res<JournalResource>>,
) {
    let report = load_application_policies(&mut registry, journal.as_deref());
    log_report(&report);
}

/// Run the active Twin's separate startup policy over its authored overrides.
pub fn sync_policies_on_twin_added(
    trigger: On<lunco_workspace::TwinAdded>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut registry: ResMut<ScriptedPolicyRegistry>,
    journal: Option<Res<JournalResource>>,
    #[cfg(feature = "native-plugins")] mut native_plugins: ResMut<
        crate::native_plugins::NativeTwinPlugins,
    >,
) {
    let twin_id = trigger.event().twin;
    let Some(workspace) = workspace.as_deref() else {
        return;
    };
    if workspace.active_twin != Some(twin_id) {
        return;
    }
    let Some(twin) = workspace.twin(twin_id) else {
        return;
    };
    let event = if registry.active_twin == Some(twin_id) {
        "reload"
    } else {
        if let Some(previous_id) = registry.active_twin {
            if let Some(previous) = workspace.twin(previous_id) {
                registry.lifecycle =
                    invoke_twin_lifecycle("close", previous_id, &previous.root, &registry.status);
            }
        }
        #[cfg(feature = "native-plugins")]
        native_plugins.unload();
        "startup"
    };
    #[cfg(feature = "native-plugins")]
    log_native_plugin_report(native_plugins.load_for_twin(twin_id, twin));
    let report = load_twin_policies(&twin.root, &mut registry, journal.as_deref());
    registry.active_twin = Some(twin_id);
    registry.lifecycle = invoke_twin_lifecycle(event, twin_id, &twin.root, &report);
    log_report(&report);
}

/// Run the active Twin's close policy, then restore the application layer.
pub fn wind_down_policies_on_twin_closed(
    trigger: On<lunco_workspace::TwinClosed>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    roots: Option<Res<lunco_assets_core::twin_source::TwinRoots>>,
    mut registry: ResMut<ScriptedPolicyRegistry>,
    journal: Option<Res<JournalResource>>,
    mut commands: Commands,
    #[cfg(feature = "native-plugins")] mut native_plugins: ResMut<
        crate::native_plugins::NativeTwinPlugins,
    >,
) {
    if !trigger.event().was_active {
        return;
    }
    let twin_id = trigger.event().twin;
    registry.lifecycle =
        invoke_twin_lifecycle("close", twin_id, &trigger.event().root, &registry.status);
    #[cfg(feature = "native-plugins")]
    native_plugins.unload();
    wind_down_twin_policies(&mut registry, journal.as_deref());
    registry.active_twin = None;
    registry.status = registry.application_status.clone();
    let next = workspace.as_deref().and_then(|workspace| {
        workspace.active_twin.and_then(|twin_id| {
            workspace
                .twin(twin_id)
                .map(|twin| (twin_id, twin.root.clone()))
        })
    });
    if let Some((twin_id, root)) = next {
        #[cfg(feature = "native-plugins")]
        if let Some(twin) = workspace
            .as_deref()
            .and_then(|workspace| workspace.twin(twin_id))
        {
            log_native_plugin_report(native_plugins.load_for_twin(twin_id, twin));
        }
        let report = load_twin_policies(&root, &mut registry, journal.as_deref());
        registry.active_twin = Some(twin_id);
        registry.lifecycle = invoke_twin_lifecycle("startup", twin_id, &root, &report);
        log_report(&report);
        if let Some(name) = roots
            .as_deref()
            .and_then(|roots| roots.name_for_root(&root).ok().flatten())
        {
            commands.trigger(lunco_assets_runtime::TwinAssetMounted {
                twin: twin_id,
                name,
            });
        }
    }
}

#[cfg(feature = "native-plugins")]
fn log_native_plugin_report(report: crate::native_plugins::NativePluginLoadReport) {
    for failure in report.failed {
        warn!("[native-hook-plugin] {failure}");
    }
}

/// Activate a deterministic, convergent rhai merge policy and switch the
/// journal to it only after compilation succeeds.
pub fn activate_scripted_merge_policy(
    journal: &JournalResource,
    hook_id: &str,
    entry: &str,
    source: &str,
) -> Result<(), String> {
    lunco_hooks_rhai::register_rhai_hook(hook_id, entry, source, true)?;
    journal.with_write(|journal| {
        journal.set_merge_strategy(MergeStrategy::Scripted(hook_id.to_string()))
    });
    Ok(())
}

/// Revert a journal to its built-in convergent `(lamport, author)` order.
pub fn use_default_merge_policy(journal: &JournalResource) {
    journal.with_write(|journal| journal.set_merge_strategy(MergeStrategy::Default));
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_twin_journal::MergeStrategy;

    fn policy(seam: &str, source: &str, deterministic: bool) -> PolicyDef {
        PolicyDef {
            seam: seam.into(),
            entry: "cmp".into(),
            source: source.into(),
            deterministic,
        }
    }

    #[test]
    fn merge_policy_switches_and_reverts_the_journal() {
        use lunco_twin_journal::{AuthorId, TwinId};
        let journal = JournalResource::new(TwinId::new("policy"), AuthorId::new("me"));
        apply_policy(&policy(MERGE_SEAM, "fn cmp(a,b){0}", true), Some(&journal)).unwrap();
        journal.with_read(|journal| {
            assert_eq!(
                *journal.merge_strategy(),
                MergeStrategy::Scripted(MERGE_SEAM.into())
            );
        });
        retract_policy(MERGE_SEAM, Some(&journal));
        journal.with_read(|journal| assert_eq!(*journal.merge_strategy(), MergeStrategy::Default));
    }
}
