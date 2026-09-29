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
use lunco_core_runtime::{
    AsyncWorkAdmission, AsyncWorkKey, AsyncWorkKind, AsyncWorkPriority, SimulationProgress,
    SimulationProgressKey,
};
use lunco_doc_bevy::JournalResource;
use lunco_hooks::{HookValue, ScriptHook};
use lunco_twin_journal::MergeStrategy;
use rhai::{Dynamic, Engine, ImmutableString};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The reserved policy seam that drives the journal's convergent merge order.
pub const MERGE_SEAM: &str = "journal.merge.order";

/// The one application startup seam. Its authored function returns policy
/// identities in the desired installation order.
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
    description: "Select application policy order or install active Twin manifest policies.",
    signature: [policies: ArrayOfMap],
    output: ArrayOfMap,
    deterministic: false,
    required: false,
    installable: false,
}

lunco_hooks::declare_hook! {
    id: APPLICATION_ASSET_HOOK,
    owner: "lunco-assets-runtime",
    description: "Select scene dataset text assets and project application or Twin text assets into generic UI contributions.",
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
    /// Hook ids skipped because their optional owner is absent from this
    /// feature-selected runtime composition.
    pub unavailable: Vec<String>,
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
    /// Owner-supplied cycle and generation for the lifecycle invocation.
    pub runtime_context: Option<lunco_core::RuntimeExecutionContext>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TwinPolicyLifecycleEvent {
    Startup,
    Reload,
}

impl TwinPolicyLifecycleEvent {
    fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Reload => "reload",
        }
    }
}

struct TwinPolicyLoadCompletion {
    twin: lunco_workspace::TwinId,
    root: PathBuf,
    operation: u64,
    result: Result<Option<lunco_assets_runtime::scripting::LoadedPolicyBundle>, String>,
}

struct ActiveTwinPolicyLoad {
    twin: lunco_workspace::TwinId,
    root: PathBuf,
    operation: u64,
    progress_key: SimulationProgressKey,
    event: TwinPolicyLifecycleEvent,
    notify_mounted_after_ready: bool,
    inputs: Option<lunco_assets_runtime::scripting::TwinPolicySetInputs>,
    work_key: Option<AsyncWorkKey>,
    capacity_revision: Option<u64>,
}

/// One active Twin policy-source preparation through shared bounded admission.
#[derive(Resource, Default)]
pub struct PendingTwinPolicyLoad {
    active: Option<ActiveTwinPolicyLoad>,
    next_operation: u64,
    completions: Arc<Mutex<Vec<TwinPolicyLoadCompletion>>>,
}

struct PreparedApplicationPolicyHook {
    definition: PolicyDef,
    policy_file: String,
    hook: Result<PreparedApplicationPolicyCallable, String>,
}

struct PreparedApplicationPolicyCallable {
    hook: Arc<lunco_hooks_rhai::RhaiHook>,
    deterministic: bool,
}

struct ValidatedPolicyDefinition {
    deterministic: bool,
    arity: usize,
}

struct PreparedApplicationPolicyBundle {
    bundle: lunco_assets_runtime::scripting::LoadedPolicyBundle,
    hooks: HashMap<String, PreparedApplicationPolicyHook>,
    startup_policy_order: Vec<String>,
    unavailable: Vec<String>,
}

/// One application policy bundle prepared on the async-compute pool before its
/// lifecycle-bound activation in `PreStartup`.
#[derive(Resource)]
pub struct PendingApplicationPolicyPreparation(
    Mutex<Option<std::sync::mpsc::Receiver<Result<PreparedApplicationPolicyBundle, String>>>>,
);

impl PendingApplicationPolicyPreparation {
    fn receive(&self) -> Result<PreparedApplicationPolicyBundle, String> {
        let receiver = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .ok_or_else(|| "application policy preparation was already consumed".to_owned())?;
        receiver.recv().map_err(|_| {
            "application policy preparation worker ended without a result".to_owned()
        })?
    }
}

impl PendingTwinPolicyLoad {
    fn allocate_operation(&mut self) -> Option<u64> {
        let operation = self.next_operation.checked_add(1)?;
        self.next_operation = operation;
        Some(operation)
    }
}

/// The active Twin's asset-mounted event waits here while its authored policy
/// sources are being prepared off-thread.
#[derive(Resource, Default)]
pub struct PendingTwinAssetMounted(Option<lunco_assets_runtime::TwinAssetMounted>);

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

fn install_startup_policy(
    id: &str,
    definition: PolicyDef,
    policy_file: String,
    state: &Mutex<StartupInstallState>,
    reusable: &HashMap<String, PolicyDef>,
    journal: Option<&JournalResource>,
    prepared_hook: Option<Result<PreparedApplicationPolicyCallable, String>>,
) -> Result<(), String> {
    let mut state = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.attempted.insert(id.to_owned());
    if definition.seam != id {
        let error = format!(
            "startup selected '{id}' but resolved manifest policy '{}'",
            definition.seam
        );
        state.failed.push(format!("{id}: {error}"));
        return Err(error);
    }
    if reusable.get(&definition.seam) == Some(&definition)
        && lunco_hooks::get(&definition.seam).is_some()
    {
        state.installed.push(definition.seam.clone());
        state.reused.insert(definition.seam.clone());
        state.definitions.push(definition);
        return Ok(());
    }
    let result = match prepared_hook {
        Some(prepared_hook) => {
            prepared_hook.and_then(|hook| apply_prepared_policy(&definition, journal, hook))
        }
        None => apply_policy(&definition, journal),
    };
    match result {
        Ok(()) => {
            lunco_hooks::bind_policy(
                definition.seam.clone(),
                lunco_hooks::HookPolicyBinding {
                    policy_file,
                    policy_entry: definition.entry.clone(),
                },
            );
            state.installed.push(definition.seam.clone());
            state.definitions.push(definition);
            Ok(())
        }
        Err(error) => {
            state.failed.push(format!("{}: {error}", definition.seam));
            Err(error)
        }
    }
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

fn select_application_policy_order(
    startup: &lunco_assets_runtime::scripting::LoadedStartup,
    policy_ids: &[String],
) -> Result<Vec<String>, String> {
    let startup_hook =
        lunco_hooks_rhai::RhaiHook::compile(&startup.source, startup.spec.entry.clone()).map_err(
            |error| {
                format!(
                    "startup policy '{}' failed to compile ({}): {error}",
                    startup.policy_file, startup.spec.entry
                )
            },
        )?;
    let policies = HookValue::Array(
        policy_ids
            .iter()
            .map(|id| HookValue::map([("hook", HookValue::str(id.clone()))]))
            .collect(),
    );
    let output = startup_hook
        .invoke(&lunco_hooks::HookInvocation::unclassified(&[policies]))
        .map_err(|error| {
            format!(
                "startup policy '{}' failed ({}): {error}",
                startup.policy_file, startup.spec.entry
            )
        })?;
    let HookValue::Array(selected) = output else {
        return Err(format!(
            "startup policy '{}' returned no policy order array",
            startup.policy_file
        ));
    };
    let order = selected
        .into_iter()
        .map(|record| {
            record
                .get("hook")
                .and_then(HookValue::as_str)
                .map(str::to_owned)
                .ok_or_else(|| {
                    format!(
                        "startup policy '{}' returned an order entry without a string hook id",
                        startup.policy_file
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    validate_application_policy_order(&order, policy_ids)?;
    Ok(order)
}

fn validate_application_policy_order(
    order: &[String],
    available_ids: &[String],
) -> Result<(), String> {
    let mut expected = HashSet::with_capacity(available_ids.len());
    for id in available_ids {
        if !expected.insert(id.as_str()) {
            return Err(format!(
                "application manifest contains duplicate policy hook '{id}'"
            ));
        }
    }
    let mut selected = HashSet::with_capacity(order.len());
    for id in order {
        if !expected.contains(id.as_str()) {
            return Err(format!(
                "application startup selected unknown policy '{id}'"
            ));
        }
        if !selected.insert(id.as_str()) {
            return Err(format!(
                "application startup selected policy '{id}' more than once"
            ));
        }
    }
    if selected.len() != expected.len() {
        return Err(format!(
            "application startup selected {} of {} available policies",
            selected.len(),
            expected.len()
        ));
    }
    Ok(())
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
    phase: lunco_core::RuntimePhase,
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
    invoke_twin_lifecycle_context(event, twin_id, context, phase)
}

fn invoke_twin_lifecycle_context(
    event: &str,
    twin_id: lunco_workspace::TwinId,
    context: HookValue,
    phase: lunco_core::RuntimePhase,
) -> LifecyclePolicyReport {
    if twin_id.is_unassigned() {
        let error = "Twin lifecycle invocation requires an assigned Twin identity";
        warn!("[policy] {error}");
        return LifecyclePolicyReport {
            event: event.to_owned(),
            status: "fault".into(),
            error: Some(error.into()),
            ..Default::default()
        };
    }
    let runtime_context = lunco_core::RuntimeExecutionContext {
        route: Some(lunco_core::RuntimeRoute::twin(
            lunco_core::RuntimeCycle::Lifecycle,
            twin_id.raw(),
        )),
        phase,
        clock: lunco_core::RuntimeClock::None,
        time_seconds: None,
        delta_seconds: None,
        sequence: None,
        producer: None,
    };
    match lunco_hooks::invoke_with_context(
        TWIN_LIFECYCLE_HOOK,
        &[HookValue::str(event), context],
        runtime_context,
    ) {
        None => LifecyclePolicyReport {
            event: event.to_owned(),
            status: "unavailable".into(),
            runtime_context: Some(runtime_context),
            ..Default::default()
        },
        Some(Ok(value @ HookValue::Map(_))) => LifecyclePolicyReport {
            event: event.to_owned(),
            status: "ok".into(),
            result: Some(value),
            runtime_context: Some(runtime_context),
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
                runtime_context: Some(runtime_context),
                ..Default::default()
            }
        }
        Some(Err(error)) => {
            warn!("[policy] Twin lifecycle event '{event}' failed: {error}");
            LifecyclePolicyReport {
                event: event.to_owned(),
                status: "fault".into(),
                error: Some(error.to_string()),
                runtime_context: Some(runtime_context),
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

/// Let the application asset policy select dataset text assets required by the
/// newly completed scene.
pub fn handle_application_scene_asset_lifecycle(
    trigger: On<lunco_core::SceneTransitionCompleted>,
    mut commands: Commands,
) {
    let (event, path, root_prim) = match &trigger.event().transition {
        lunco_core::SceneTransition::Load { path, root_prim }
        | lunco_core::SceneTransition::Restart {
            path, root_prim, ..
        } => ("scene_loaded", Some(path.clone()), Some(root_prim.clone())),
        lunco_core::SceneTransition::Clear => ("scene_cleared", None, None),
    };
    let payload = HookValue::map([
        ("path", path.clone().map_or(HookValue::Unit, HookValue::str)),
        (
            "root_prim",
            root_prim.map_or(HookValue::Unit, HookValue::str),
        ),
    ]);
    apply_application_asset_policy(
        event,
        "application:scene-assets",
        None,
        None,
        path.unwrap_or_default(),
        payload,
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
    let result = lunco_hooks::invoke_unclassified(
        APPLICATION_ASSET_HOOK,
        &[HookValue::str(event.to_owned()), context],
    );
    let actions = match result {
        None => Ok((Vec::new(), Vec::new())),
        Some(Ok(value @ HookValue::Map(_))) => parse_application_asset_actions(&value),
        Some(Ok(value)) => Err(format!(
            "asset lifecycle policy returned {}, expected map",
            value.type_name()
        )),
        Some(Err(error)) => Err(error.to_string()),
    };
    let (menus, dataset_text_artifacts) = match actions {
        Ok(actions) => actions,
        Err(error) => {
            warn!("[application-assets] `{provider}` policy failed: {error}");
            (Vec::new(), Vec::new())
        }
    };
    commands.trigger(
        lunco_scripting_rhai_core::ui_bridge::ScriptUiRequest::WorkbenchMenus {
            provider: provider.to_owned(),
            twin_id: twin_id.map(lunco_workspace::TwinId::raw),
            menus,
        },
    );
    for id in dataset_text_artifacts {
        commands.trigger(lunco_assets_runtime::ReadDatasetTextArtifact { id });
    }
}

fn parse_application_asset_actions(
    value: &HookValue,
) -> Result<
    (
        Vec<lunco_scripting_rhai_core::ui_bridge::ScriptWorkbenchMenu>,
        Vec<String>,
    ),
    String,
> {
    let menus = parse_script_workbench_menus(value)?;
    let artifacts = match value.get("dataset_text_artifacts") {
        None => Vec::new(),
        Some(HookValue::Array(items)) => {
            if items.len() > 64 {
                return Err("one policy result may request at most 64 dataset text assets".into());
            }
            let mut seen = HashSet::new();
            let mut artifacts = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                let HookValue::Str(id) = item else {
                    return Err(format!(
                        "dataset_text_artifacts[{index}] must be a dataset id string, got {}",
                        item.type_name()
                    ));
                };
                if id.trim().is_empty() {
                    return Err(format!("dataset_text_artifacts[{index}] must not be empty"));
                }
                if seen.insert(id.clone()) {
                    artifacts.push(id.clone());
                }
            }
            artifacts
        }
        Some(other) => {
            return Err(format!(
                "`dataset_text_artifacts` must be an array, got {}",
                other.type_name()
            ));
        }
    };
    Ok((menus, artifacts))
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
            ));
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
            ));
        }
    };
    let enabled = match hook_field(fields, "enabled") {
        None => true,
        Some(HookValue::Bool(enabled)) => *enabled,
        Some(other) => {
            return Err(format!(
                "{path}.enabled must be boolean, got {}",
                other.type_name()
            ));
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
                    ));
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
        HookValue::UInt(value) => i64::try_from(*value)
            .map(TelemetryValue::I64)
            .map_err(|_| format!("{path} exceeds the telemetry integer range"))?,
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
    pending_load: Res<PendingTwinPolicyLoad>,
    mut pending_mount: ResMut<PendingTwinAssetMounted>,
) {
    let twin_id = trigger.event().twin;
    let Some(workspace) = workspace.as_deref() else {
        return;
    };
    let policy_pending = pending_load
        .active
        .as_ref()
        .is_some_and(|loading| loading.twin == twin_id);
    if workspace.active_twin == Some(twin_id)
        && (policy_pending || registry.active_twin != Some(twin_id))
    {
        pending_mount.0 = Some(trigger.event().clone());
        return;
    }
    plan_twin_asset_loading_for(trigger.event(), workspace, &mut registry, &mut pending);
}

fn plan_twin_asset_loading_for(
    mounted: &lunco_assets_runtime::TwinAssetMounted,
    workspace: &lunco_workspace::WorkspaceResource,
    registry: &mut ScriptedPolicyRegistry,
    pending: &mut PendingTwinPolicyCommands,
) {
    let twin_id = mounted.twin;
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
        ("name", HookValue::str(mounted.name.clone())),
        ("root", HookValue::str(twin.root.display().to_string())),
        (
            "active",
            HookValue::Bool(workspace.active_twin == Some(twin_id)),
        ),
        ("manifest", manifest),
        ("files", files),
    ]);
    let report = invoke_twin_lifecycle_context(
        "assets_mounted",
        twin_id,
        context,
        lunco_core::RuntimePhase::Event,
    );
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
        let context = lunco_core::RuntimeExecutionContext {
            route: Some(lunco_core::RuntimeRoute::application(
                lunco_core::RuntimeCycle::Command,
            )),
            phase: lunco_core::RuntimePhase::Command,
            clock: lunco_core::RuntimeClock::Application,
            time_seconds: world
                .get_resource::<lunco_core_runtime::ApplicationCadence>()
                .map(|cadence| cadence.command.elapsed_secs),
            delta_seconds: world
                .get_resource::<lunco_core_runtime::ApplicationCadence>()
                .and_then(|cadence| cadence.command.interval_secs),
            sequence: world
                .get_resource::<lunco_core_runtime::ApplicationCadence>()
                .map(|cadence| cadence.command.sequence),
            producer: None,
        };
        let _scope = lunco_scripting_bridge_core::WorldScope::enter(world, context);
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

    // Reconcile only the seams whose effective implementation changed. Compare
    // callable identity rather than wrapper registration: restoring a retained
    // lower layer wraps its same callable in a fresh registration.
    for id in ids {
        let Some(_definition) = active.get(&id) else {
            lunco_hooks::unregister(&id);
            lunco_hooks::unbind_policy(&id);
            continue;
        };
        let desired = layer_hook(registry, &id);
        let current = lunco_hooks::get(&id);
        let same_registration =
            desired
                .as_ref()
                .zip(current.as_ref())
                .is_some_and(|(desired, current)| {
                    desired.0.backend == current.backend
                        && desired.0.deterministic == current.deterministic
                        && Arc::ptr_eq(&desired.0.hook, &current.hook)
                });
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
                    let policy_id = id.to_string();
                    let definition = PolicyDef {
                        seam: policy_id.clone(),
                        entry: entry.to_string(),
                        source: source.to_string(),
                        deterministic,
                    };
                    match install_startup_policy(
                        &policy_id,
                        definition,
                        policy_file.to_string(),
                        &callback_state,
                        &reusable,
                        callback_journal.as_ref(),
                        None,
                    ) {
                        Ok(()) => startup_operation_result(&policy_id, true, None),
                        Err(error) => {
                            startup_operation_result(&policy_id, false, Some(error.as_str()))
                        }
                    }
                },
            );
        },
    )?;
    let policies = HookValue::Array(loaded.iter().map(policy_value).collect());
    let result = startup_hook
        .invoke(&lunco_hooks::HookInvocation::unclassified(&[policies]))
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
    let deterministic = validate_policy_definition(def)?.deterministic;
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

fn prepare_application_policy_bundle() -> Result<PreparedApplicationPolicyBundle, String> {
    let bundle = {
        let _span = bevy::log::info_span!("application_policy_source_prepare_offthread").entered();
        lunco_assets_runtime::scripting::active_policy_bundle()?
    };
    let _span = bevy::log::info_span!("application_policy_compile_offthread").entered();
    prepare_application_policy_bundle_from(bundle)
}

fn prepare_application_policy_bundle_for_validation()
-> Result<PreparedApplicationPolicyBundle, String> {
    let bundle = lunco_assets_runtime::scripting::active_policy_bundle()?;
    prepare_application_policy_bundle_from(bundle)
}

fn prepare_application_policy_bundle_from(
    mut bundle: lunco_assets_runtime::scripting::LoadedPolicyBundle,
) -> Result<PreparedApplicationPolicyBundle, String> {
    let startup = bundle
        .startup
        .as_ref()
        .ok_or_else(|| "application policy set has no startup entry".to_owned())?;
    let (available_policies, unavailable) = {
        let _span = bevy::log::info_span!("application_policy_filter_manifest_offthread").entered();
        retain_runtime_policies(std::mem::take(&mut bundle.policies))
    };
    bundle.policies = available_policies;
    let available_policy_ids = bundle
        .policies
        .iter()
        .map(|policy| policy.spec.hook.clone())
        .collect::<Vec<_>>();
    let hooks = bundle
        .policies
        .iter()
        .map(|policy| {
            let definition = PolicyDef {
                seam: policy.spec.hook.clone(),
                entry: policy.spec.entry.clone(),
                source: policy.source.clone(),
                deterministic: policy.spec.deterministic,
            };
            let hook =
                lunco_hooks_rhai::RhaiHook::compile(&policy.source, policy.spec.entry.clone())
                    .and_then(|hook| {
                        let _span = bevy::log::info_span!(
                            "application_policy_validate_prepared_hook_offthread"
                        )
                        .entered();
                        let validated = validate_policy_definition(&definition)?;
                        if !hook.supports_arity(validated.arity) {
                            return Err(format!(
                                "Rhai hook '{}' has no '{}' overload accepting {} argument(s)",
                                definition.seam, definition.entry, validated.arity
                            ));
                        }
                        Ok(PreparedApplicationPolicyCallable {
                            hook: Arc::new(hook),
                            deterministic: validated.deterministic,
                        })
                    });
            (
                definition.seam.clone(),
                PreparedApplicationPolicyHook {
                    definition,
                    policy_file: policy.policy_file.clone(),
                    hook,
                },
            )
        })
        .collect();
    let startup_policy_order = select_application_policy_order(startup, &available_policy_ids)?;
    Ok(PreparedApplicationPolicyBundle {
        bundle,
        hooks,
        startup_policy_order,
        unavailable,
    })
}

/// Begin the single application-policy preparation while the remaining app
/// plugins are being installed. The authored function selects hook order on
/// the worker; `PreStartup` validates and commits the prepared hooks before any
/// Startup consumer runs.
pub fn prepare_application_policies_offthread(app: &mut App) {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    #[cfg(not(target_arch = "wasm32"))]
    match bevy::tasks::AsyncComputeTaskPool::try_get() {
        Some(pool) => {
            pool.spawn(async move {
                let _ = sender.send(prepare_application_policy_bundle());
            })
            .detach();
        }
        None => {
            let _ = sender.send(Err(
                "AsyncComputeTaskPool must be initialized before application policy preparation"
                    .into(),
            ));
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = sender.send(Err(
            "application policy preparation requires native async asset access".into(),
        ));
    }
    app.insert_resource(PendingApplicationPolicyPreparation(Mutex::new(Some(
        receiver,
    ))));
}

fn validate_policy_definition(def: &PolicyDef) -> Result<ValidatedPolicyDefinition, String> {
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
    Ok(ValidatedPolicyDefinition {
        deterministic,
        arity: contract.parameters.len(),
    })
}

fn apply_prepared_policy(
    def: &PolicyDef,
    journal: Option<&JournalResource>,
    prepared: PreparedApplicationPolicyCallable,
) -> Result<(), String> {
    let hook: Arc<dyn ScriptHook> = prepared.hook;
    lunco_hooks::register(lunco_hooks::RegisteredHook {
        id: def.seam.clone(),
        backend: "rhai".into(),
        deterministic: prepared.deterministic,
        hook,
    });
    if def.seam == MERGE_SEAM {
        if let Some(journal) = journal {
            journal.with_write(|journal| {
                journal.set_merge_strategy(MergeStrategy::Scripted(def.seam.clone()))
            });
        }
    }
    Ok(())
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
    // Keep unrelated active hooks registered while the USD layer changes. This
    // projection runs on source-asset events as well as stage edits; clearing
    // the global hook registry first made unchanged application policies
    // briefly unavailable to concurrent source admission.
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

fn retain_runtime_policies(
    policies: Vec<lunco_assets_runtime::scripting::LoadedPolicy>,
) -> (
    Vec<lunco_assets_runtime::scripting::LoadedPolicy>,
    Vec<String>,
) {
    let mut available = Vec::with_capacity(policies.len());
    let mut unavailable = Vec::new();
    for policy in policies {
        if !runtime_policy_available(&policy) {
            unavailable.push(policy.spec.hook);
        } else {
            available.push(policy);
        }
    }
    (available, unavailable)
}

fn runtime_policy_available(policy: &lunco_assets_runtime::scripting::LoadedPolicy) -> bool {
    !policy.spec.skip_when_hook_unavailable || lunco_hooks::descriptor(&policy.spec.hook).is_some()
}

fn activate_prepared_application_policy_order(
    order: &[String],
    loaded: &[lunco_assets_runtime::scripting::LoadedPolicy],
    mut prepared_hooks: HashMap<String, PreparedApplicationPolicyHook>,
    journal: Option<&JournalResource>,
) -> Result<StartupInstallState, String> {
    let available_ids = loaded
        .iter()
        .map(|policy| policy.spec.hook.clone())
        .collect::<Vec<_>>();
    {
        let _span = bevy::log::info_span!("application_policy_validate_order").entered();
        validate_application_policy_order(order, &available_ids)?;
    }
    {
        let _span = bevy::log::info_span!("application_policy_validate_prepared_hooks").entered();
        for policy in loaded {
            let Some(prepared) = prepared_hooks.get(&policy.spec.hook) else {
                return Err(format!(
                    "application startup has no prepared hook for '{}'",
                    policy.spec.hook
                ));
            };
            if prepared.definition.seam != policy.spec.hook
                || prepared.definition.entry != policy.spec.entry
                || prepared.definition.deterministic != policy.spec.deterministic
                || prepared.policy_file != policy.policy_file
            {
                return Err(format!(
                    "prepared application hook '{}' no longer matches its manifest record",
                    policy.spec.hook
                ));
            }
        }
    }

    let state = Mutex::new(StartupInstallState::default());
    let reusable = HashMap::new();
    {
        let _span = bevy::log::info_span!("application_policy_install_prepared_hooks").entered();
        for id in order {
            let prepared = prepared_hooks
                .remove(id)
                .ok_or_else(|| format!("application startup has no prepared hook for '{id}'"))?;
            let _ = install_startup_policy(
                id,
                prepared.definition,
                prepared.policy_file,
                &state,
                &reusable,
                journal,
                Some(prepared.hook),
            );
        }
    }
    Ok(state
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
}

fn report_for_application_policies(
    scope: impl Into<String>,
    application: lunco_assets_runtime::scripting::LoadedPolicyBundle,
    registry: &mut ScriptedPolicyRegistry,
    journal: Option<&JournalResource>,
    prepared_hooks: HashMap<String, PreparedApplicationPolicyHook>,
    startup_policy_order: Vec<String>,
    unavailable: Vec<String>,
) -> PolicyLoadReport {
    let scope = scope.into();
    {
        let _span = bevy::log::info_span!("application_policy_clear_registry").entered();
        clear_active_policies(registry, journal);
    }
    if application.startup.is_none() {
        let report = PolicyLoadReport {
            scope,
            error: Some("application policy set has no startup entry".into()),
            ..Default::default()
        };
        registry.application_status = report.clone();
        registry.status = report.clone();
        return report;
    }
    let application_loaded = application.policies;
    let required = application_loaded
        .iter()
        .filter(|policy| policy.spec.required)
        .map(|policy| policy.spec.hook.clone())
        .collect::<HashSet<_>>();
    let application_run = match activate_prepared_application_policy_order(
        &startup_policy_order,
        &application_loaded,
        prepared_hooks,
        journal,
    ) {
        Ok(run) => run,
        Err(error) => {
            let report = PolicyLoadReport {
                scope,
                unavailable,
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
    {
        let _span = bevy::log::info_span!("application_policy_publish_registry").entered();
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
    }

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
        unavailable,
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
            unavailable: registry.application_status.unavailable.clone(),
            ..Default::default()
        };
        registry.status = report.clone();
        return report;
    };
    let (twin_policies, twin_unavailable) = retain_runtime_policies(twin.policies);
    let mut unavailable = registry.application_status.unavailable.clone();
    for hook in twin_unavailable {
        if !unavailable.contains(&hook) {
            unavailable.push(hook);
        }
    }
    let twin_ids = twin_policies
        .iter()
        .map(|policy| policy.spec.hook.clone())
        .collect::<HashSet<_>>();
    let required = twin_policies
        .iter()
        .filter(|policy| policy.spec.required)
        .map(|policy| policy.spec.hook.clone())
        .collect::<HashSet<_>>();
    let Some(startup) = twin.startup else {
        registry.twin_scope_ids = twin_ids;
        rebuild_active_policy_registry(registry, journal);
        let report = PolicyLoadReport {
            scope,
            unavailable,
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
    let run = run_startup_policy(startup, &twin_policies, journal, &reusable);
    let (installed, failed, error, definitions) = match run {
        Ok(run) => {
            registry.twin_hooks = run
                .installed
                .iter()
                .filter_map(|id| lunco_hooks::get(id).map(|hook| (id.clone(), hook)))
                .collect();
            registry.twin_bindings = twin_policies
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
        unavailable,
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
    if !report.unavailable.is_empty() {
        info!(
            "[policy] {} skipped optional hooks without a linked owner: {}",
            report.scope,
            report.unavailable.join(", ")
        );
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
    activate_prepared_application_policies(
        prepare_application_policy_bundle_for_validation(),
        registry,
        journal,
    )
}

fn activate_prepared_application_policies(
    prepared: Result<PreparedApplicationPolicyBundle, String>,
    registry: &mut ScriptedPolicyRegistry,
    journal: Option<&JournalResource>,
) -> PolicyLoadReport {
    let previous = registry.policies.clone();
    let report = match prepared {
        Ok(prepared) => report_for_application_policies(
            "application",
            prepared.bundle,
            registry,
            journal,
            prepared.hooks,
            prepared.startup_policy_order,
            prepared.unavailable,
        ),
        Err(error) => report_load_error("application", error, registry, journal),
    };
    if registry.policies != previous {
        registry.revision = registry.revision.wrapping_add(1);
    }
    report
}

/// Commit a worker-prepared Twin policy layer on the scripting lifecycle owner.
fn activate_twin_policies(
    root: &Path,
    bundle: Result<Option<lunco_assets_runtime::scripting::LoadedPolicyBundle>, String>,
    registry: &mut ScriptedPolicyRegistry,
    journal: Option<&JournalResource>,
) -> PolicyLoadReport {
    let previous = registry.policies.clone();
    let twin_bundle = match bundle {
        Ok(bundle) => bundle,
        Err(error) => {
            let report = report_load_error("Twin", error, registry, journal);
            if registry.policies != previous {
                registry.revision = registry.revision.wrapping_add(1);
            }
            return report;
        }
    };
    let twin_bundle = twin_bundle.map(|mut bundle| {
        bundle.policies = coalesce_loaded_policies(bundle.policies);
        bundle
    });
    let report = report_for_twin_policies(
        format!("Twin {}", root.display()),
        twin_bundle,
        registry,
        journal,
    );
    if registry.policies != previous {
        registry.revision = registry.revision.wrapping_add(1);
    }
    report
}

/// Startup system for the application policy set.
pub fn load_application_policies_on_startup(
    mut registry: ResMut<ScriptedPolicyRegistry>,
    journal: Option<Res<JournalResource>>,
    preparation: Res<PendingApplicationPolicyPreparation>,
) {
    let prepared = {
        let _span = bevy::log::info_span!("application_policy_prestartup_wait").entered();
        preparation.receive()
    };
    let report = {
        let _span = bevy::log::info_span!("application_policy_activation").entered();
        activate_prepared_application_policies(prepared, &mut registry, journal.as_deref())
    };
    log_report(&report);
}

/// Run the active Twin's separate startup policy over its authored overrides.
pub fn sync_policies_on_twin_added(
    trigger: On<lunco_workspace::TwinAdded>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    roots: Option<Res<lunco_assets_core::twin_source::TwinRoots>>,
    mut registry: ResMut<ScriptedPolicyRegistry>,
    mut pending_load: ResMut<PendingTwinPolicyLoad>,
    mut admission: ResMut<AsyncWorkAdmission>,
    mut progress: ResMut<SimulationProgress>,
    mut pending_mount: ResMut<PendingTwinAssetMounted>,
    mut pending_commands: ResMut<PendingTwinPolicyCommands>,
    journal: Option<Res<JournalResource>>,
    mut commands: Commands,
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
    start_twin_policy_load(
        twin_id,
        twin,
        workspace,
        false,
        &mut pending_load,
        &mut admission,
        &mut progress,
        &mut pending_mount,
        &mut pending_commands,
        &mut registry,
        journal.as_deref(),
        roots.as_deref(),
        &mut commands,
        #[cfg(feature = "native-plugins")]
        &mut native_plugins,
    );
}

fn start_twin_policy_load(
    twin_id: lunco_workspace::TwinId,
    twin: &lunco_workspace::Twin,
    workspace: &lunco_workspace::WorkspaceResource,
    notify_mounted_after_ready: bool,
    pending_load: &mut PendingTwinPolicyLoad,
    admission: &mut AsyncWorkAdmission,
    progress: &mut SimulationProgress,
    pending_mount: &mut PendingTwinAssetMounted,
    pending_commands: &mut PendingTwinPolicyCommands,
    registry: &mut ScriptedPolicyRegistry,
    journal: Option<&JournalResource>,
    roots: Option<&lunco_assets_core::twin_source::TwinRoots>,
    commands: &mut Commands<'_, '_>,
    #[cfg(feature = "native-plugins")]
    native_plugins: &mut crate::native_plugins::NativeTwinPlugins,
) {
    if let Some(existing) = pending_load.active.as_mut() {
        if existing.twin == twin_id && existing.root == twin.root {
            existing.notify_mounted_after_ready |= notify_mounted_after_ready;
            return;
        }
    }

    let Some(operation) = pending_load.allocate_operation() else {
        if let Some(previous) = pending_load.active.take() {
            if let Some(key) = previous.work_key {
                admission.cancel_queued(key);
            }
            progress.release(previous.progress_key);
        }
        lunco_core::trigger_runtime_error(
            commands,
            "twin-policy-operation-exhausted",
            "Twin policy preparation operation id exhausted",
        );
        return;
    };

    // There is one active request slot. Withdraw an older queued request and
    // fence any already-dispatched result with the new operation id.
    if let Some(previous) = pending_load.active.take() {
        if let Some(key) = previous.work_key {
            admission.cancel_queued(key);
        }
        progress.release(previous.progress_key);
    }
    if pending_mount
        .0
        .as_ref()
        .is_some_and(|mounted| mounted.twin != twin_id)
    {
        pending_mount.0 = None;
    }

    let event = if registry.active_twin == Some(twin_id) {
        TwinPolicyLifecycleEvent::Reload
    } else {
        if let Some(previous_id) = registry.active_twin {
            if let Some(previous) = workspace.twin(previous_id) {
                registry.lifecycle = invoke_twin_lifecycle(
                    "close",
                    previous_id,
                    &previous.root,
                    &registry.status,
                    lunco_core::RuntimePhase::Stop,
                );
            }
        }
        #[cfg(feature = "native-plugins")]
        native_plugins.unload();
        let previous = registry.policies.clone();
        wind_down_twin_policies(registry, journal);
        if registry.policies != previous {
            registry.revision = registry.revision.wrapping_add(1);
        }
        registry.active_twin = None;
        registry.status = registry.application_status.clone();
        TwinPolicyLifecycleEvent::Startup
    };

    let inputs = lunco_assets_runtime::scripting::twin_policy_set_inputs(twin);
    if inputs.is_empty() {
        complete_twin_policy_load(
            twin_id,
            twin,
            workspace,
            event,
            notify_mounted_after_ready,
            Ok(None),
            registry,
            journal,
            roots,
            pending_mount,
            pending_commands,
            commands,
            #[cfg(feature = "native-plugins")]
            native_plugins,
        );
        return;
    }

    #[cfg(target_arch = "wasm32")]
    {
        let _ = (operation, admission);
        // The browser build has no shared worker dispatcher. Preserve its
        // existing WebStorage-backed policy loading until a browser worker
        // transport can own source parsing.
        let bundle = lunco_assets_runtime::scripting::twin_policy_set(inputs);
        complete_twin_policy_load(
            twin_id,
            twin,
            workspace,
            event,
            notify_mounted_after_ready,
            bundle,
            registry,
            journal,
            roots,
            pending_mount,
            pending_commands,
            commands,
            #[cfg(feature = "native-plugins")]
            native_plugins,
        );
        return;
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let progress_key = SimulationProgressKey::twin_policy_preparation(operation);
        progress.acquire(progress_key, "Preparing active Twin policy sources");
        let active = ActiveTwinPolicyLoad {
            twin: twin_id,
            root: twin.root.clone(),
            operation,
            progress_key,
            event,
            notify_mounted_after_ready,
            inputs: Some(inputs),
            work_key: None,
            capacity_revision: None,
        };
        pending_load.active = Some(active);
        if let Some(active) = pending_load.active.as_mut() {
            submit_twin_policy_load(active, &pending_load.completions, admission);
        }
    }
}

fn submit_twin_policy_load(
    active: &mut ActiveTwinPolicyLoad,
    completions: &Arc<Mutex<Vec<TwinPolicyLoadCompletion>>>,
    admission: &mut AsyncWorkAdmission,
) {
    let Some(inputs) = active.inputs.as_ref().cloned() else {
        return;
    };
    let identity = u128::from(active.twin.raw());
    let key = AsyncWorkKey::new(
        AsyncWorkKind::TwinPolicyPreparation,
        active.twin.raw(),
        identity,
        0,
        active.operation,
    );
    let completion_queue = Arc::clone(completions);
    let twin = active.twin;
    let root = active.root.clone();
    let operation = active.operation;
    let job = move || {
        let _span = bevy::log::info_span!("twin_policy_source_prepare_offthread").entered();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            lunco_assets_runtime::scripting::twin_policy_set(inputs)
        }))
        .unwrap_or_else(|_| Err("Twin policy source preparation panicked".to_owned()));
        completion_queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(TwinPolicyLoadCompletion {
                twin,
                root,
                operation,
                result,
            });
    };

    let capacity_revision = admission.capacity_revision();
    match admission.submit(AsyncWorkPriority::SimulationRequired, key, job) {
        Ok(()) => {
            active.inputs = None;
            active.work_key = Some(key);
            active.capacity_revision = None;
        }
        Err(lunco_core_runtime::AsyncWorkRejection::QueueFull) => {
            active.capacity_revision = Some(capacity_revision);
        }
        Err(rejection) => {
            active.inputs = None;
            active.capacity_revision = None;
            let detail = match rejection {
                lunco_core_runtime::AsyncWorkRejection::DuplicateKey => {
                    "Twin policy preparation operation was already submitted"
                }
                lunco_core_runtime::AsyncWorkRejection::QueueFull => {
                    "Twin policy preparation queue is full"
                }
                lunco_core_runtime::AsyncWorkRejection::NativeDispatcherUnavailable => {
                    "Twin policy preparation requires a worker transport on this host"
                }
            };
            completions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(TwinPolicyLoadCompletion {
                    twin,
                    root: active.root.clone(),
                    operation: active.operation,
                    result: Err(detail.to_owned()),
                });
        }
    }
}

fn complete_twin_policy_load(
    twin_id: lunco_workspace::TwinId,
    twin: &lunco_workspace::Twin,
    workspace: &lunco_workspace::WorkspaceResource,
    event: TwinPolicyLifecycleEvent,
    notify_mounted_after_ready: bool,
    bundle: Result<Option<lunco_assets_runtime::scripting::LoadedPolicyBundle>, String>,
    registry: &mut ScriptedPolicyRegistry,
    journal: Option<&JournalResource>,
    roots: Option<&lunco_assets_core::twin_source::TwinRoots>,
    pending_mount: &mut PendingTwinAssetMounted,
    pending_commands: &mut PendingTwinPolicyCommands,
    commands: &mut Commands<'_, '_>,
    #[cfg(feature = "native-plugins")]
    native_plugins: &mut crate::native_plugins::NativeTwinPlugins,
) {
    let _span = bevy::log::info_span!("twin_policy_activate").entered();
    #[cfg(feature = "native-plugins")]
    log_native_plugin_report(native_plugins.load_for_twin(twin_id, twin));

    let report = activate_twin_policies(&twin.root, bundle, registry, journal);
    registry.active_twin = Some(twin_id);
    registry.lifecycle = invoke_twin_lifecycle(
        event.as_str(),
        twin_id,
        &twin.root,
        &report,
        lunco_core::RuntimePhase::Start,
    );
    log_report(&report);

    if pending_mount
        .0
        .as_ref()
        .is_some_and(|mounted| mounted.twin == twin_id)
    {
        if let Some(mounted) = pending_mount.0.take() {
            plan_twin_asset_loading_for(&mounted, workspace, registry, pending_commands);
        }
    } else if notify_mounted_after_ready {
        if let Some(name) = roots.and_then(|roots| roots.name_for_root(&twin.root).ok().flatten()) {
            commands.trigger(lunco_assets_runtime::TwinAssetMounted {
                twin: twin_id,
                name,
            });
        }
    }
}

/// Commit the active Twin's prepared policy sources at the lifecycle boundary.
pub fn poll_pending_twin_policy_load(
    mut pending_load: ResMut<PendingTwinPolicyLoad>,
    mut admission: ResMut<AsyncWorkAdmission>,
    mut progress: ResMut<SimulationProgress>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    roots: Option<Res<lunco_assets_core::twin_source::TwinRoots>>,
    mut registry: ResMut<ScriptedPolicyRegistry>,
    mut pending_mount: ResMut<PendingTwinAssetMounted>,
    mut pending_commands: ResMut<PendingTwinPolicyCommands>,
    journal: Option<Res<JournalResource>>,
    mut commands: Commands,
    #[cfg(feature = "native-plugins")] mut native_plugins: ResMut<
        crate::native_plugins::NativeTwinPlugins,
    >,
) {
    let active_operation = pending_load.active.as_ref().map(|active| active.operation);
    let Some(operation) = active_operation else {
        pending_load
            .completions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        return;
    };
    let completion = {
        let mut completions = pending_load
            .completions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let completed = completions
            .iter()
            .position(|completion| completion.operation == operation)
            .map(|index| completions.remove(index));
        completions.retain(|completion| completion.operation == operation);
        completed
    };
    let Some(completion) = completion else {
        let capacity_revision = admission.capacity_revision();
        let retry = pending_load.active.as_ref().is_some_and(|active| {
            active.inputs.is_some() && active.capacity_revision != Some(capacity_revision)
        });
        if retry {
            let completions = Arc::clone(&pending_load.completions);
            if let Some(active) = pending_load.active.as_mut() {
                submit_twin_policy_load(active, &completions, &mut admission);
            }
        }
        return;
    };
    let Some(loading) = pending_load.active.take() else {
        return;
    };
    if completion.operation != loading.operation
        || completion.twin != loading.twin
        || completion.root != loading.root
    {
        progress.release(loading.progress_key);
        lunco_core::trigger_runtime_error(
            &mut commands,
            "twin-policy-completion-mismatch",
            "Twin policy preparation returned a mismatched operation identity",
        );
        return;
    }
    let Some(workspace) = workspace.as_deref() else {
        progress.release(loading.progress_key);
        return;
    };
    if workspace.active_twin != Some(loading.twin) {
        progress.release(loading.progress_key);
        return;
    }
    let Some(twin) = workspace.twin(loading.twin) else {
        progress.release(loading.progress_key);
        return;
    };
    if twin.root != loading.root {
        progress.release(loading.progress_key);
        return;
    }

    complete_twin_policy_load(
        loading.twin,
        twin,
        workspace,
        loading.event,
        loading.notify_mounted_after_ready,
        completion.result,
        &mut registry,
        journal.as_deref(),
        roots.as_deref(),
        &mut pending_mount,
        &mut pending_commands,
        &mut commands,
        #[cfg(feature = "native-plugins")]
        &mut native_plugins,
    );
    progress.release(loading.progress_key);
}

/// Run the active Twin's close policy, then restore the application layer.
pub fn wind_down_policies_on_twin_closed(
    trigger: On<lunco_workspace::TwinClosed>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    roots: Option<Res<lunco_assets_core::twin_source::TwinRoots>>,
    mut registry: ResMut<ScriptedPolicyRegistry>,
    mut pending_load: ResMut<PendingTwinPolicyLoad>,
    mut admission: ResMut<AsyncWorkAdmission>,
    mut progress: ResMut<SimulationProgress>,
    mut pending_mount: ResMut<PendingTwinAssetMounted>,
    mut pending_commands: ResMut<PendingTwinPolicyCommands>,
    journal: Option<Res<JournalResource>>,
    mut commands: Commands,
    #[cfg(feature = "native-plugins")] mut native_plugins: ResMut<
        crate::native_plugins::NativeTwinPlugins,
    >,
) {
    let twin_id = trigger.event().twin;
    if pending_load
        .active
        .as_ref()
        .is_some_and(|loading| loading.twin == twin_id)
    {
        if let Some(loading) = pending_load.active.take() {
            if let Some(key) = loading.work_key {
                admission.cancel_queued(key);
            }
            progress.release(loading.progress_key);
        }
    }
    if pending_mount
        .0
        .as_ref()
        .is_some_and(|mounted| mounted.twin == twin_id)
    {
        pending_mount.0 = None;
    }
    if !trigger.event().was_active {
        return;
    }
    registry.lifecycle = invoke_twin_lifecycle(
        "close",
        twin_id,
        &trigger.event().root,
        &registry.status,
        lunco_core::RuntimePhase::Stop,
    );
    #[cfg(feature = "native-plugins")]
    native_plugins.unload();
    let previous = registry.policies.clone();
    wind_down_twin_policies(&mut registry, journal.as_deref());
    if registry.policies != previous {
        registry.revision = registry.revision.wrapping_add(1);
    }
    registry.active_twin = None;
    registry.status = registry.application_status.clone();
    let Some(workspace) = workspace.as_deref() else {
        return;
    };
    let next = workspace
        .active_twin
        .and_then(|next_twin| workspace.twin(next_twin).map(|twin| (next_twin, twin)));
    if let Some((twin_id, twin)) = next {
        start_twin_policy_load(
            twin_id,
            twin,
            workspace,
            true,
            &mut pending_load,
            &mut admission,
            &mut progress,
            &mut pending_mount,
            &mut pending_commands,
            &mut registry,
            journal.as_deref(),
            roots.as_deref(),
            &mut commands,
            #[cfg(feature = "native-plugins")]
            &mut native_plugins,
        );
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
