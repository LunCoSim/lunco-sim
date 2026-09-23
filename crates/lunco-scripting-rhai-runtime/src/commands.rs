//! One-shot script-execution commands.
//!
//! `RunRhai`, `RunRhaiTool`, and `RunRhaiToolHook` are typed `#[Command]`s —
//! discoverable on every transport (HTTP API, MCP, scripts) like any other
//! command. They are registered only with this Rhai runtime, so an accepted
//! command always has a corresponding handler.
//!
//! The handler returns `Result<Ack, String>`. API callers receive the completed
//! stdout or error on the same deferred request; in-process `cmd()` callers
//! continue to use the internal command-result substrate.
//!
//! Interpreter-specific commands belong to that interpreter's runtime package;
//! the generic scripting host owns backend-neutral lifecycle/document commands.

use bevy::prelude::*;
#[cfg(feature = "rhai")]
use lunco_api::executor::PendingApiRequest;
use lunco_command_contracts::{Ack, OpId};
use lunco_core::ActiveCommandId;
use lunco_core::register_commands;
use lunco_core::{Command, on_command};
#[cfg(feature = "rhai")]
use lunco_doc::DocumentId;
#[cfg(feature = "rhai")]
use lunco_scripting::ScriptRegistry;
#[cfg(feature = "rhai")]
use lunco_scripting::doc::{
    ScenarioParameters, ScenarioReloadPolicy, ScriptDocument, ScriptLanguage, ScriptOp,
    ScriptedModel,
};
#[cfg(feature = "rhai")]
use lunco_scripting_bridge_core as bridge_core;
#[cfg(feature = "rhai")]
use lunco_scripting_rhai_world::world_bridge::{PendingWorldScript, PendingWorldScripts};
#[cfg(feature = "rhai")]
use lunco_telemetry_core::TelemetryValue;

/// Run a rhai snippet against the live world — the scripting escape hatch when
/// no typed command covers what you need.
///
/// The result arrives on the next `Update`: rhai needs full `World` access,
/// which an observer cannot hold, so the handler enqueues the snippet and the
/// exclusive `drain_world_scripts` system runs it before answering the
/// deferred API request with the real stdout. `Update` is intentional because
/// kinematic celestial warp freezes `FixedUpdate`.
#[cfg(feature = "rhai")]
#[Command(default)]
pub struct RunRhai {
    /// rhai source to evaluate. The scripting prelude is in scope.
    pub code: String,
}

/// Invoke a registered Rhai tool with a typed value.
///
/// This is the structured counterpart to [`RunRhai`]. It is intended for
/// engine adapters such as scene click tools: the payload crosses the Bevy
/// command queue as the shared [`TelemetryValue`] model and becomes a native
/// Rhai value inside the scripting backend. No source snippet or JSON literal
/// is used to carry the payload.
#[cfg(feature = "rhai")]
#[Command(default)]
pub struct RunRhaiTool {
    /// Registered tool namespace, for example `recover` or `waypoint_editor`.
    pub tool: String,
    /// Structured argument passed as the single `on_click(context)` argument.
    pub args: TelemetryValue,
}

/// Invoke any one-argument hook exposed by a registered Rhai tool.
///
/// This is the generic interaction seam used by authored pointer policies and
/// menus. The hook name is validated against the tool registry before it is
/// queued; the payload remains a typed [`TelemetryValue`] until the Rhai
/// adapter creates its native value.
#[cfg(feature = "rhai")]
#[Command(default)]
pub struct RunRhaiToolHook {
    /// Registered tool namespace, for example `waypoint_editor`.
    pub tool: String,
    /// One-argument function in that namespace, without `/1`.
    pub hook: String,
    /// Structured argument passed to the hook.
    pub args: TelemetryValue,
}

/// One source-level edit for a script document. The document host performs
/// UTF-8/range validation and records the inverse operation for undo.
#[cfg(feature = "rhai")]
#[derive(Reflect, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum ScriptApiOp {
    /// Replace the complete source buffer.
    ReplaceSource {
        /// New UTF-8 source text.
        source: String,
    },
    /// Replace one UTF-8 byte range.
    EditText {
        /// Inclusive-start byte offset.
        range_start: u64,
        /// Exclusive-end byte offset.
        range_end: u64,
        /// Replacement UTF-8 source.
        replacement: String,
    },
}

/// Apply one grouped source edit to an explicit script document.
#[cfg(feature = "rhai")]
#[Command(default)]
pub struct ApplyScriptOps {
    /// Explicit script document id.
    pub doc_id: DocumentId,
    /// Ordered source operations committed as one undo/journal group.
    pub ops: Vec<ScriptApiOp>,
    /// Optional optimistic cursor from `InspectScriptDocument`.
    #[serde(default)]
    pub parent_generation: Option<u64>,
}

#[cfg(feature = "rhai")]
#[on_command(ApplyScriptOps)]
fn on_apply_script_ops(
    trigger: On<ApplyScriptOps>,
    mut registry: ResMut<ScriptRegistry>,
) -> Result<Ack, String> {
    let request = trigger.event();
    if request.doc_id.is_unassigned() {
        return Err("ApplyScriptOps requires an explicit doc_id".to_owned());
    }
    let Some(host) = registry.documents.get_mut(&request.doc_id) else {
        return Err(format!(
            "ApplyScriptOps: unknown script document {}",
            request.doc_id
        ));
    };
    if host.document().language != ScriptLanguage::Rhai {
        return Err(format!(
            "ApplyScriptOps: document {} is not a Rhai source",
            request.doc_id
        ));
    }
    if let Some(parent) = request.parent_generation {
        let current = host.generation();
        if current != parent {
            return Err(format!(
                "ApplyScriptOps: stale parent generation for doc {}: expected {}, current {}",
                request.doc_id, parent, current
            ));
        }
    }
    let mut ops = Vec::with_capacity(request.ops.len());
    for (index, op) in request.ops.iter().enumerate() {
        let op = match op {
            ScriptApiOp::ReplaceSource { source } => ScriptOp::SetSource(source.clone()),
            ScriptApiOp::EditText {
                range_start,
                range_end,
                replacement,
            } => {
                let start = usize::try_from(*range_start).map_err(|_| {
                    format!("ApplyScriptOps: start offset at index {index} exceeds usize")
                })?;
                let end = usize::try_from(*range_end).map_err(|_| {
                    format!("ApplyScriptOps: end offset at index {index} exceeds usize")
                })?;
                if start > end {
                    return Err(format!(
                        "ApplyScriptOps: text range at index {index} is not ordered"
                    ));
                }
                ScriptOp::EditText {
                    range: start..end,
                    replacement: replacement.clone(),
                }
            }
        };
        ops.push(op);
    }
    if ops.is_empty() {
        return Err("ApplyScriptOps requires at least one operation".to_owned());
    }
    let count = ops.len();
    let ack = host
        .apply_group_against(request.parent_generation, ops)
        .map_err(|reject| format!("ApplyScriptOps: {reject}"))?;
    Ok(Ack {
        data: Some(lunco_api_core::api_value!({
            "doc_id": request.doc_id.raw(),
            "operations": count,
        })),
        ..ack
    })
}

// rhai runs with full World access (`cmd`/`world_pos`/`get`/...), which an
// observer can't hold. So the handler ENQUEUES the snippet under the active
// request id; the exclusive `drain_world_scripts` system runs it next Update
// and records the real stdout.
#[cfg(feature = "rhai")]
#[on_command(RunRhai)]
fn on_run_rhai(
    _t: On<RunRhai>,
    active: Res<ActiveCommandId>,
    pending_request: Res<PendingApiRequest>,
    mut pending: ResMut<PendingWorldScripts>,
    guard: Option<Res<lunco_core_session::SyncApplyGuard>>,
) -> Result<Ack, String> {
    let id = active.get().unwrap_or(0);
    // §3.4: gate the snippet's cmd()s against the submitting session. `Some`
    // only when this RunRhai arrived from the wire (a remote peer); `None` for a
    // local / host-issued snippet → host-trusted (ungated).
    let authority = guard.and_then(|g| g.0);
    let correlation_id =
        (pending_request.correlation_id != 0).then_some(pending_request.correlation_id);
    pending.queue.push(PendingWorldScript::Code {
        id,
        code: cmd.code.clone(),
        authority,
        correlation_id,
    });
    Ok(Ack::with_data(
        OpId::new(),
        lunco_api_core::api_value!({ "status": "queued" }),
    ))
}

#[cfg(feature = "rhai")]
#[on_command(RunRhaiTool)]
fn on_run_rhai_tool(
    trigger: On<RunRhaiTool>,
    active: Res<ActiveCommandId>,
    pending_request: Res<PendingApiRequest>,
    mut pending: ResMut<PendingWorldScripts>,
    guard: Option<Res<lunco_core_session::SyncApplyGuard>>,
) -> Result<Ack, String> {
    let cmd = trigger.event();
    if !lunco_tools::has_function(&cmd.tool, lunco_tools::UI_CLICK_FN) {
        return Err(format!(
            "Rhai tool '{}' is not a registered on_click/1 tool",
            cmd.tool
        ));
    }
    let id = active.get().unwrap_or(0);
    let authority = guard.and_then(|g| g.0);
    let correlation_id =
        (pending_request.correlation_id != 0).then_some(pending_request.correlation_id);
    pending.queue.push(PendingWorldScript::Tool {
        id,
        tool: cmd.tool.clone(),
        hook: "on_click".to_string(),
        args: cmd.args.clone(),
        authority,
        correlation_id,
    });
    Ok(Ack::with_data(
        OpId::new(),
        lunco_api_core::api_value!({ "status": "queued" }),
    ))
}

#[cfg(feature = "rhai")]
#[on_command(RunRhaiToolHook)]
fn on_run_rhai_tool_hook(
    trigger: On<RunRhaiToolHook>,
    active: Res<ActiveCommandId>,
    pending_request: Res<PendingApiRequest>,
    mut pending: ResMut<PendingWorldScripts>,
    guard: Option<Res<lunco_core_session::SyncApplyGuard>>,
) -> Result<Ack, String> {
    let cmd = trigger.event();
    if cmd.hook.is_empty()
        || !cmd
            .hook
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err(format!(
            "invalid Rhai tool hook '{}'; expected an identifier",
            cmd.hook
        ));
    }
    let signature = format!("{}/1", cmd.hook);
    if !lunco_tools::has_function(&cmd.tool, &signature) {
        return Err(format!(
            "Rhai tool '{}' has no {} handler",
            cmd.tool, signature
        ));
    }
    let id = active.get().unwrap_or(0);
    let authority = guard.and_then(|g| g.0);
    let correlation_id =
        (pending_request.correlation_id != 0).then_some(pending_request.correlation_id);
    pending.queue.push(PendingWorldScript::Tool {
        id,
        tool: cmd.tool.clone(),
        hook: cmd.hook.clone(),
        args: cmd.args.clone(),
        authority,
        correlation_id,
    });
    Ok(Ack::with_data(
        OpId::new(),
        lunco_api_core::api_value!({ "status": "queued" }),
    ))
}

/// Attach a persistent rhai scenario to an entity — the scenario-loading entry
/// point for the API / MCP / UI / ROS2. Registers the source as a
/// `ScriptDocument` and attaches a `ScriptedModel { Rhai }` to `target`, so the
/// per-entity runtime can build a native `task(me, ctx)` tree and run optional
/// lifecycle/event hooks.
///
/// Idempotent + HOT-RELOAD: re-running on an entity that already has a scenario
/// reuses its document id and bumps the generation, so `tick_rhai_models`
/// recompiles in place (state reset) instead of leaking documents.
#[cfg(feature = "rhai")]
// `reflect_default` registers `ReflectDefault` (+ the manual `Default` below) so
// the reflect deserializer can construct commands with omitted optional fields.
// `#[Command(default)]` is not used because these commands have an explicit
// semantic default for their host and scenario lifecycle.
#[Command(reflect_default)]
pub struct RunScenario {
    #[authz_target]
    pub target: Entity,
    pub source: String,
    /// Optional typed scenario parameters (e.g.
    /// `{"speed":1.5,"target":"rover_b"}`). Rhai receives them as the
    /// explicit `ctx` argument of lifecycle/program hooks. Omitted → `{}`.
    #[serde(default)]
    #[reflect(default)]
    pub params: ScenarioParameters,
    /// Behavior of this scenario when the active scene is replaced. `retain`
    /// keeps a stable orchestration host alive; `restart` runs `on_start` again
    /// after the replacement is ready.
    #[serde(default)]
    #[reflect(default)]
    pub reload_policy: ScenarioReloadPolicy,
}

/// Attach a file-backed Rhai scenario to a scenario host. The asset is loaded
/// through the normal Bevy asset graph, so imports, Twin ownership, wasm, and
/// hot reload use the same path as USD-authored scenarios. This is the generic
/// launch seam for authored flows; no domain-specific catalog or host is
/// required.
#[cfg(feature = "rhai")]
#[Command(reflect_default)]
pub struct RunScenarioAsset {
    #[authz_target]
    /// Scenario host. Omitted requests use the active `WorldRoot`.
    #[serde(default)]
    #[reflect(default)]
    pub target: Option<Entity>,
    /// Root-qualified script asset (`lunco://...` or `twin://...`).
    pub source_asset: String,
    /// Optional typed scenario parameters. Rhai receives them as the explicit
    /// `ctx` argument of lifecycle/program hooks. Omitted → `{}`.
    #[serde(default)]
    #[reflect(default)]
    pub params: ScenarioParameters,
    /// Optional scene asset to request before the scenario starts. The scene
    /// transition remains owned by the USD scene command layer; this field
    /// only composes the generic scenario-launch request with that lifecycle.
    #[serde(default)]
    #[reflect(default)]
    pub scene_asset: String,
    /// Lifecycle behavior when the active scene is replaced.
    #[serde(default)]
    #[reflect(default)]
    pub reload_policy: ScenarioReloadPolicy,
}

#[cfg(feature = "rhai")]
impl Default for RunScenarioAsset {
    fn default() -> Self {
        Self {
            target: None,
            source_asset: String::new(),
            params: ScenarioParameters::default(),
            scene_asset: String::new(),
            reload_policy: ScenarioReloadPolicy::Retain,
        }
    }
}

#[cfg(feature = "rhai")]
impl Default for RunScenario {
    fn default() -> Self {
        Self {
            target: Entity::PLACEHOLDER,
            source: String::new(),
            params: ScenarioParameters::default(),
            reload_policy: ScenarioReloadPolicy::Retain,
        }
    }
}

#[cfg(feature = "rhai")]
#[on_command(RunScenario)]
fn on_run_scenario(
    _t: On<RunScenario>,
    entities: Query<Entity>,
    world_root: Query<Entity, With<lunco_spatial::WorldRoot>>,
    mut registry: ResMut<ScriptRegistry>,
    q_existing: Query<&ScriptedModel>,
    guard: Option<Res<lunco_core_session::SyncApplyGuard>>,
    mut commands: Commands,
) -> Result<Ack, String> {
    let target = resolve_scenario_target(Some(cmd.target), &entities, &world_root)?;
    let (doc_id_raw, generation) = attach_rhai_scenario(
        target,
        cmd.source.clone(),
        cmd.params.clone(),
        // A `RunScenario` carries SOURCE TEXT, not a location — there is no asset
        // id to anchor a relative import against.
        None,
        ScenarioSourceMode::UserEdit,
        false,
        cmd.reload_policy,
        guard.and_then(|g| g.0),
        &mut registry,
        &q_existing,
        &mut commands,
    )?;
    Ok(Ack::with_data(
        OpId::new(),
        lunco_api_core::api_value!({ "document_id": doc_id_raw, "generation": generation }),
    ))
}

#[cfg(feature = "rhai")]
#[on_command(RunScenarioAsset)]
fn on_run_scenario_asset(
    trigger: On<RunScenarioAsset>,
    entities: Query<Entity>,
    world_root: Query<Entity, With<lunco_spatial::WorldRoot>>,
    asset_server: Res<AssetServer>,
    guard: Option<Res<lunco_core_session::SyncApplyGuard>>,
    mut commands: Commands,
) -> Result<Ack, String> {
    let cmd = trigger.event();
    let path = lunco_assets_core::engine_asset_uri(&cmd.source_asset);
    if path.is_empty() {
        return Err("RunScenarioAsset: source_asset must not be empty".to_string());
    }
    let target = resolve_scenario_target(cmd.target, &entities, &world_root)?;
    let scene = if cmd.scene_asset.trim().is_empty() {
        None
    } else {
        let scene = lunco_assets_core::engine_asset_uri(&cmd.scene_asset);
        if scene.is_empty() {
            return Err("RunScenarioAsset: scene_asset is not a valid asset path".to_string());
        }
        Some(scene)
    };
    let handle = asset_server.load::<lunco_scripting_rhai_world::source_asset::RhaiSource>(path);
    commands.entity(target).try_insert(PendingScenarioAsset {
        handle,
        params: cmd.params.clone(),
        reload_policy: cmd.reload_policy,
        authority: guard.and_then(|g| g.0),
    });
    if let Some(scene) = scene {
        // This is an intent, not a direct USD load. The USD scene owner still
        // resolves/composes the stage and publishes the completion edge that
        // opens scenario execution.
        commands.trigger(lunco_core::SceneTransitionIntent::load(scene, ""));
    }
    Ok(Ack::with_data(
        OpId::new(),
        lunco_api_core::api_value!({ "status": "queued" }),
    ))
}

#[cfg(feature = "rhai")]
fn resolve_scenario_target(
    requested: Option<Entity>,
    entities: &Query<Entity>,
    world_root: &Query<Entity, With<lunco_spatial::WorldRoot>>,
) -> Result<Entity, String> {
    if let Some(requested) = requested {
        if requested != Entity::PLACEHOLDER {
            if entities.get(requested).is_ok() {
                return Ok(requested);
            }
            return Err(format!("scenario target {requested:?} does not exist"));
        }
    }
    world_root
        .iter()
        .next()
        .ok_or_else(|| "RunScenarioAsset: no WorldRoot exists for the default host".to_string())
}

/// Register a rhai source as a `ScriptDocument` and attach a `ScriptedModel` to
/// `target`, reusing the doc id (hot-reload, generation bump) if one already
/// exists. Shared by `RunScenario` and `RunTimeline`. Returns `(doc_id, generation)`.
#[cfg(feature = "rhai")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScenarioSourceMode {
    /// An explicit editor/API source edit. It is a ScriptOp and therefore
    /// undoable, journaled, and replayable through the script document host.
    UserEdit,
    /// Source projected from an authored USD prim or an asset file. That owner
    /// already journals/persists the source change, so this refreshes the
    /// script projection without creating a duplicate ScriptOp.
    External,
    /// A generated runtime program such as a timeline executor. It changes the
    /// running program but is not an authored source edit.
    Runtime,
}

#[cfg(feature = "rhai")]
fn attach_rhai_scenario(
    target: Entity,
    source: String,
    params: ScenarioParameters,
    // Canonical asset id this source was loaded from (`twin://ep1/main.rhai`), or
    // `None` for a source that is not file-backed — an inline USD `info:sourceCode`,
    // a `RunScenario` string off the wire, a generated timeline executor. `None`
    // is a real, expected state, not a missing value: such a script has no
    // location, so a RELATIVE `import` in it cannot be anchored and must fail
    // rather than silently resolve against some invented root.
    asset_id: Option<String>,
    source_mode: ScenarioSourceMode,
    // Whether the scenario document is owned by the authored scene and must
    // be wound down with that scene.
    scene_owned: bool,
    reload_policy: ScenarioReloadPolicy,
    authority: Option<lunco_command_contracts::SessionId>,
    registry: &mut ScriptRegistry,
    q_existing: &Query<&ScriptedModel>,
    commands: &mut Commands,
) -> Result<(u64, u64), String> {
    // Reuse the doc id if a scenario is already attached (hot-reload), else mint.
    let existing = q_existing.get(target).ok().and_then(|m| m.document_id);
    let doc_id_raw = existing.unwrap_or_else(|| DocumentId::fresh().raw());

    if let Some(id) = existing {
        let doc_id = DocumentId::new(id);
        let source_changed = registry
            .documents
            .get(&doc_id)
            .is_some_and(|host| host.document().source != source);
        if source_changed {
            match source_mode {
                ScenarioSourceMode::UserEdit => {
                    registry
                        .apply(
                            doc_id,
                            lunco_scripting::doc::ScriptOp::SetSource(source.clone()),
                        )
                        .map_err(|reject| format!("scenario source edit rejected: {reject:?}"))?;
                }
                ScenarioSourceMode::External | ScenarioSourceMode::Runtime => {
                    registry.reload_external_source(doc_id, &source);
                }
            }
        }
        let host = registry
            .documents
            .get_mut(&doc_id)
            .ok_or_else(|| format!("script document {doc_id} disappeared during attach"))?;
        host.document_mut().asset_id = asset_id;
    } else {
        let mut doc = ScriptDocument::new(doc_id_raw, ScriptLanguage::Rhai, source);
        // Script IDENTITY is carried on the document because the runtime
        // recompiles from it and uses the asset id as the relative-import
        // anchor. A missing id is meaningful for inline/generated sources.
        doc.asset_id = asset_id;
        registry.insert_document(DocumentId::new(doc_id_raw), doc);
    }

    let parameters_revision = q_existing
        .get(target)
        .map(|model| model.parameters_revision.wrapping_add(1))
        .unwrap_or(0);
    commands.entity(target).try_insert((
        ScriptedModel {
            document_id: Some(doc_id_raw),
            language: Some(ScriptLanguage::Rhai),
            reload_policy,
            parameters: params,
            parameters_revision,
            ..default()
        },
        // §3.4: the session this scenario's cmd()s are gated against. Always
        // (re)inserted so a hot-reload relaunch refreshes it; `None` = ungated.
        lunco_scripting::scenario::ScriptAuthority(authority),
    ));
    if scene_owned {
        commands
            .entity(target)
            .try_insert(lunco_scripting::SceneOwnedScript);
    } else {
        commands
            .entity(target)
            .remove::<lunco_scripting::SceneOwnedScript>();
    }

    let generation = registry
        .documents
        .get(&DocumentId::new(doc_id_raw))
        .map(|host| host.document().generation)
        .ok_or_else(|| format!("script document {doc_id_raw} missing after attach"))?;
    Ok((doc_id_raw, generation))
}

/// A generic file-backed scenario waiting for its root asset and import graph
/// to finish loading. It is deliberately a component on the target entity, so
/// the request follows the same lifecycle and ownership boundary as the
/// scenario it will replace.
#[cfg(feature = "rhai")]
#[derive(Component, Debug, Clone)]
pub struct PendingScenarioAsset {
    pub handle: Handle<lunco_scripting_rhai_world::source_asset::RhaiSource>,
    pub params: ScenarioParameters,
    pub reload_policy: ScenarioReloadPolicy,
    pub authority: Option<lunco_command_contracts::SessionId>,
}

/// Resolve [`RunScenarioAsset`] requests once the root and all imported source
/// assets are ready, then use the same attach funnel as inline/API scenarios.
#[cfg(feature = "rhai")]
pub fn attach_requested_scenarios(
    q: Query<(Entity, &PendingScenarioAsset)>,
    assets: Res<Assets<lunco_scripting_rhai_world::source_asset::RhaiSource>>,
    asset_server: Res<AssetServer>,
    mut registry: ResMut<ScriptRegistry>,
    q_existing: Query<&ScriptedModel>,
    mut commands: Commands,
) {
    for (entity, request) in q.iter() {
        let root_failed = asset_server.load_state(&request.handle).is_failed();
        let dependencies_failed = asset_server
            .recursive_dependency_load_state(&request.handle)
            .is_failed();
        if root_failed || dependencies_failed {
            error!(
                "[rhai] failed to load requested scenario asset for {entity:?}; \
                 root_failed={root_failed}, dependencies_failed={dependencies_failed}"
            );
            commands.entity(entity).remove::<PendingScenarioAsset>();
            continue;
        }
        if !asset_server.is_loaded_with_dependencies(&request.handle) {
            continue;
        }
        let Some(source) = assets.get(&request.handle) else {
            continue;
        };
        let Some(asset_id) = asset_server
            .get_path(&request.handle)
            .map(|path| lunco_assets_core::asset_path::anchor_of(&path))
        else {
            error!("[rhai] requested scenario asset for {entity:?} has no resolved identity");
            commands.entity(entity).remove::<PendingScenarioAsset>();
            continue;
        };
        let request = request.clone();
        match attach_rhai_scenario(
            entity,
            source.text.clone(),
            request.params,
            Some(asset_id),
            ScenarioSourceMode::External,
            false,
            request.reload_policy,
            request.authority,
            &mut registry,
            &q_existing,
            &mut commands,
        ) {
            Ok(_) => {}
            Err(error) => error!("[rhai] scenario asset attach rejected for {entity:?}: {error}"),
        }
        commands
            .entity(entity)
            .try_insert(ScenarioAssetHandle(request.handle))
            .remove::<PendingScenarioAsset>();
    }
}

/// LOAD half of USD-embedded scenario persistence: drain entities the USD loader
/// stamped with [`lunco_core::EmbeddedScenarioSource`] (an `info:sourceCode`
/// attribute on their prim), attaching each as a running rhai scenario and
/// removing the marker. Attaches by `Entity` directly — no gid round-trip — so it
/// works the instant the prim spawns. The loader (`lunco-usd-bevy`) and this
/// runtime stay decoupled via the lunco-core marker.
#[cfg(feature = "rhai")]
pub fn attach_embedded_scenarios(
    q: Query<
        (
            Entity,
            &lunco_core::EmbeddedScenarioSource,
            Option<&ScenarioAssetId>,
        ),
        // The marker is removed after this system attaches the source, so an
        // existing model here means the authored program changed in place. The
        // attach funnel reuses its document id and advances its generation;
        // ScenarioDriver then performs the normal stop -> compile -> start
        // transition without recreating the owning USD entity.
        (),
    >,
    mut registry: ResMut<ScriptRegistry>,
    q_existing: Query<&ScriptedModel>,
    mut commands: Commands,
) {
    for (entity, embedded, asset_id) in q.iter() {
        match attach_rhai_scenario(
            entity,
            embedded.0.clone(),
            ScenarioParameters::default(),
            // Present only for the FILE-backed path below; inline `info:sourceCode`
            // authored straight into USD legitimately has no asset id.
            asset_id.map(|id| id.0.clone()),
            ScenarioSourceMode::External,
            true,
            ScenarioReloadPolicy::Retain,
            // Scene-authored (loaded by the host from USD) → host-trusted, ungated.
            None,
            &mut registry,
            &q_existing,
            &mut commands,
        ) {
            Ok(_) => {
                commands
                    .entity(entity)
                    .remove::<lunco_core::EmbeddedScenarioSource>();
            }
            Err(error) => {
                error!("[rhai] embedded scenario attach rejected for {entity:?}: {error}")
            }
        }
        commands.entity(entity).remove::<ScenarioAssetId>();
        if asset_id.is_none() {
            // Inline source replaced a previous file-backed scenario. Its old
            // root handle must leave with the old source; otherwise the Bevy
            // dependency graph would keep an unrelated Twin asset resident.
            commands.entity(entity).remove::<ScenarioAssetHandle>();
        }
    }
}

/// The canonical asset id (`twin://ep1/main.rhai`) a pending
/// [`lunco_core::EmbeddedScenarioSource`] was loaded from — the anchor a relative
/// `import` inside that script resolves against.
///
/// A SIBLING component rather than a second field on `EmbeddedScenarioSource`,
/// for two reasons:
///
/// 1. `EmbeddedScenarioSource` is also stamped by `lunco-usd-bevy` for INLINE
///    `info:sourceCode` sources, which have no asset id at all. A second field would
///    force that construction site (and the tests) to supply a value for
///    something that genuinely does not exist, and the natural filler — `""` —
///    is exactly the "empty anchor that silently resolves against another root"
///    that `ScriptSources::canonical_id` is written to avoid. Absence of the
///    component says "not file-backed" unambiguously.
/// 2. Only this crate can compute the id (it owns the load) and only this crate
///    consumes it, so the contract does not need to live in `lunco-core`.
#[cfg(feature = "rhai")]
#[derive(Component, Debug, Clone)]
pub struct ScenarioAssetId(pub String);

/// The Bevy ownership edge for a file-backed scenario.
///
/// The scenario entity owns its root `RhaiSource` handle for exactly as long as
/// the scenario is alive. `RhaiSource` owns the handles for its imported sources
/// through its dependency field, so the asset graph — rather than the global
/// synchronous import registry — controls residency and Twin teardown.
#[cfg(feature = "rhai")]
#[derive(Component, Debug, Clone)]
pub struct ScenarioAssetHandle(pub Handle<lunco_scripting_rhai_world::source_asset::RhaiSource>);

/// LOAD half for FILE-backed scenarios: entities the USD loader stamped with
/// [`lunco_core::EmbeddedScenarioPath`] (an `info:sourceAsset` attribute). Loads
/// the `.rhai` asset through the `AssetServer` (wasm-safe — no `std::fs`) and,
/// once ready, swaps the path marker for an [`lunco_core::EmbeddedScenarioSource`]
/// so [`attach_embedded_scenarios`] runs the normal attach path next. Keeps the
/// USD loader and scripting runtime decoupled via the lunco-core markers (same
/// pattern as the inline path). The `Local` map holds a strong handle per entity
/// while loading; on success ownership moves to [`ScenarioAssetHandle`].
#[cfg(feature = "rhai")]
pub fn resolve_embedded_scenario_paths(
    q: Query<
        (Entity, &lunco_core::EmbeddedScenarioPath),
        Without<lunco_core::EmbeddedScenarioSource>,
    >,
    sources: Res<Assets<lunco_scripting_rhai_world::source_asset::RhaiSource>>,
    asset_server: Res<AssetServer>,
    mut pending: Local<
        std::collections::HashMap<
            Entity,
            Handle<lunco_scripting_rhai_world::source_asset::RhaiSource>,
        >,
    >,
    mut removed: RemovedComponents<lunco_core::EmbeddedScenarioPath>,
    mut commands: Commands,
) {
    for entity in removed.read() {
        pending.remove(&entity);
    }
    for (entity, path) in q.iter() {
        let handle = pending.entry(entity).or_insert_with(|| {
            // Address the script through `lunco://` rather than stripping an
            // `assets/` prefix by hand and riding the DEFAULT source: an authored
            // `assets/foo.rhai`, a bare `foo.rhai`, and an explicit
            // `lunco://foo.rhai` must all name the same script, and only
            // `lunco-assets-core` gets to decide what that means. A ref that already
            // carries its own scheme (`twin://…`) is passed through untouched, so
            // a Twin-owned script resolves against the Twin.
            let uri = lunco_assets_core::engine_asset_uri(&path.0);
            info!(
                "[scripting] loading scenario script `{}` as `{uri}`",
                path.0
            );
            asset_server.load(uri)
        });
        let root_failed = asset_server.load_state(&*handle).is_failed();
        let dependencies_failed = asset_server
            .recursive_dependency_load_state(&*handle)
            .is_failed();
        if root_failed || dependencies_failed {
            warn!(
                "[scripting] failed to load scenario `{}` via AssetServer (root_failed={root_failed}, dependencies_failed={dependencies_failed})",
                path.0,
            );
            commands
                .entity(entity)
                .remove::<lunco_core::EmbeddedScenarioPath>()
                .remove::<ScenarioAssetHandle>();
            pending.remove(&entity);
            continue;
        }
        // `RhaiSource` declares its imports as Bevy dependencies. The source
        // text is present before those dependencies necessarily finish, but
        // attaching it at that point would let the synchronous Rhai resolver
        // race the dependency publisher. Wait on Bevy's authoritative recursive
        // state so a scenario becomes executable only with its complete import
        // graph, while the UI and the rest of the ECS keep running.
        if !asset_server.is_loaded_with_dependencies(&*handle) {
            continue;
        }
        if let Some(src) = sources.get(&*handle) {
            // Carry the script's IDENTITY alongside its text. Taken from the
            // handle's resolved `AssetPath` via `anchor_of` — the same function
            // `publish_rhai_sources` keys the import registry by — so the id a
            // scenario is compiled under is byte-identical to the id it (and its
            // siblings) are registered under. Deriving it from `path.0` by hand
            // instead would be a second canonicalization that can disagree.
            // No fallback ON PURPOSE. Deriving the id from `path.0` by hand here is
            // exactly the second canonicalization the paragraph above rules out: it
            // can disagree with the registry, and a script compiled under a
            // different id than its siblings are registered under resolves relative
            // imports to the WRONG FILE — silently, since a plausible id looks like
            // a working one. We loaded this handle by uri, so a missing path is an
            // engine invariant break, not a case to paper over: say so and skip.
            let Some(id) = asset_server
                .get_path(&*handle)
                .map(|p| lunco_assets_core::asset_path::anchor_of(&p))
            else {
                error!(
                    "[rhai] loaded script asset {:?} has no resolved AssetPath — cannot \
                     establish its identity, so it is NOT compiled (a guessed id would \
                     resolve its relative imports to the wrong file). This is a bug in \
                     asset loading, not in the script.",
                    path.0
                );
                pending.remove(&entity);
                commands
                    .entity(entity)
                    .remove::<lunco_core::EmbeddedScenarioPath>()
                    .remove::<ScenarioAssetHandle>();
                continue;
            };
            // Transfer the root handle to the scenario entity. Keeping the
            // exact resolved id is important: a guessed root spelling can
            // disagree with the AssetServer path and make relative imports bind
            // to the wrong source.
            commands
                .entity(entity)
                .try_insert((
                    lunco_core::EmbeddedScenarioSource(src.text.clone()),
                    ScenarioAssetId(id),
                    ScenarioAssetHandle(handle.clone()),
                ))
                .remove::<lunco_core::EmbeddedScenarioPath>();
            pending.remove(&entity);
        }
    }
}

/// Register (or hot-replace) a named rhai **tool library** — a reusable bundle
/// of selection / behaviour policy callable from any scenario as
/// `name::fn(...)` (see [`lunco_scripting_rhai_world::tool_libs`]). The scenario-authoring counterpart
/// to RunScenario: RunScenario attaches a program to ONE entity; this publishes
/// shared library code every scenario can call, with no Rust rebuild. Idempotent
/// + hot-reload — re-registering a name replaces it and the runtime picks it up
/// on the next tick.
#[cfg(feature = "rhai")]
#[Command(default)]
pub struct RegisterToolLibrary {
    pub name: String,
    pub source: String,
}

#[cfg(feature = "rhai")]
#[on_command(RegisterToolLibrary)]
fn on_register_tool_library(
    _t: On<RegisterToolLibrary>,
    mut scoped: ResMut<lunco_scripting_rhai_world::tool_libs::TwinToolLibraries>,
    // Optional: present only when the workspace plugin is installed. Used to
    // persist the library to the active Twin's `tools/` dir. `None` (headless /
    // no-twin) just keeps the in-memory registration.
    ws: Option<Res<lunco_workspace::WorkspaceResource>>,
    // The same asset-backed import registry used by the persistent world
    // engine. Validation below builds that production engine before publish.
    sources: Option<Res<lunco_assets_runtime::script_source::ScriptSources>>,
    // Journal handle (present once wired). Records the registration as a
    // `DomainKind::ToolLibrary` op so it syncs to peers + persists cross-platform.
    // The command isn't on the command bus, so this only fires for LOCAL
    // registrations; remote peers' registrations arrive via the replay leg
    // (which calls `register_tool_library` directly, not this command).
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
) -> Result<Ack, String> {
    lunco_scripting_rhai_core::names::validate_file_stem(&cmd.name)
        .map_err(|error| format!("RegisterToolLibrary: {error}"))?;
    let functions = lunco_scripting_rhai_world::world_bridge::validate_tool_library(
        &cmd.name,
        &cmd.source,
        sources.as_deref().cloned().unwrap_or_default(),
    )
    .map_err(|error| format!("RegisterToolLibrary: invalid Rhai library: {error}"))?;
    let active_twin = match ws.as_deref() {
        Some(workspace) => {
            let id = workspace
                .active_twin
                .ok_or_else(|| "RegisterToolLibrary: no active Twin".to_string())?;
            if workspace.twin(id).is_none() {
                return Err("RegisterToolLibrary: active Twin is not registered".to_string());
            }
            Some(id)
        }
        None => None,
    };
    // Persist first. A failed write must not publish a library that the Twin
    // cannot recover on its next open.
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(twin_id) = active_twin {
        let root = ws
            .as_ref()
            .and_then(|workspace| workspace.twin(twin_id))
            .map(|twin| twin.root.clone())
            .expect("active Twin was validated above");
        lunco_scripting_rhai_world::tool_libs::save_tool_library_file(
            &root,
            &cmd.name,
            &cmd.source,
        )
        .map_err(|error| format!("RegisterToolLibrary: could not persist Twin file: {error}"))?;
    }
    if let Some(twin_id) = active_twin {
        scoped.ensure_active(twin_id);
        scoped
            .register(twin_id, &cmd.name, &cmd.source)
            .map_err(|error| format!("RegisterToolLibrary: {error}"))?;
    } else {
        // Explicit headless/session scope. A present Workspace always
        // produces an active Twin above; it never falls back to this path.
        lunco_scripting_rhai_world::tool_libs::register_tool_library(&cmd.name, &cmd.source);
    }
    if let Some(journal) = journal.as_ref() {
        crate::registration_journal::record_tool_library(journal, &cmd.name, &cmd.source);
    }
    let function_details = functions
        .iter()
        .map(|signature| {
            let (name, arity) = signature
                .rsplit_once('/')
                .map(|(name, arity)| (name, arity.parse::<usize>().unwrap_or_default()))
                .unwrap_or((signature.as_str(), 0));
            lunco_api_core::api_value!({ "name": name, "arity": arity })
        })
        .collect::<Vec<lunco_api_core::ApiValue>>();
    let scope = active_twin
        .map(|twin| lunco_api_core::api_value!({ "kind": "twin", "id": twin.raw() }))
        .unwrap_or_else(|| lunco_api_core::api_value!({ "kind": "session" }));
    Ok(Ack::with_data(
        OpId::new(),
        lunco_api_core::api_value!({
            "name": cmd.name.clone(),
            "active_twin": active_twin.map(|twin| twin.raw()),
            "scope": scope,
            "registry_generation": lunco_scripting_rhai_world::tool_libs::generation(),
            "functions": function_details,
            "callable": true,
            "diagnostics": [],
            "libraries": lunco_scripting_rhai_world::tool_libs::library_names(),
        }),
    ))
}

/// Run a declarative **mission timeline** on an entity — Layer 2 of the
/// sequencer. The timeline is a typed parameter map containing a `steps` array
/// and optional `name`. The handler attaches a fixed Rhai executor and passes
/// the structured timeline through `ctx`; no data is generated into source.
/// It attaches via the same path as `RunScenario` — so hot-reload, per-entity
/// state, and `TASK_COMPLETE`/`TASK_FAILED` telemetry all come from the native
/// task driver.
///
/// Step vocabulary (see prelude `timeline_step`): `{move_to,speed,radius}`,
/// `{move_to_entity,speed,radius}`, `{possess}`, `{brake,secs}`,
/// `{cmd,params}`, `{emit,value}`, `{wait}`, `{wait_event}`. Each step must
/// contain exactly one operation field; the operation word is the timeline
/// discriminator and common fields are validated separately below.
#[cfg(feature = "rhai")]
#[Command]
pub struct RunTimeline {
    #[authz_target]
    pub target: Entity,
    /// Structured timeline object with required `steps` and optional `name`.
    pub timeline: ScenarioParameters,
}

/// Validate the typed timeline structure and return its step count.
#[cfg(feature = "rhai")]
pub(crate) fn timeline_step_count(timeline: &ScenarioParameters) -> Result<usize, String> {
    let values = timeline.as_map();
    for key in values.keys() {
        if key != "steps" && key != "name" {
            return Err(format!("timeline has unknown field `{key}`"));
        }
    }
    if let Some(name) = values.get("name")
        && !matches!(name, TelemetryValue::String(_))
    {
        return Err("timeline `name` must be a string".to_string());
    }
    let steps = match values.get("steps") {
        Some(TelemetryValue::Array(steps)) => steps,
        Some(_) => return Err("timeline `steps` must be an array".to_string()),
        None => return Err("timeline object needs a `steps` array".to_string()),
    };
    for (index, step) in steps.iter().enumerate() {
        let TelemetryValue::Map(object) = step else {
            return Err(format!("step {index} must be an object"));
        };
        let active: Vec<&str> = TimelineOperation::ALL
            .iter()
            .map(|operation| operation.name())
            .filter(|name| object.contains_key(*name))
            .collect();
        if active.is_empty() {
            return Err(format!("step {index} has no recognized operation"));
        }
        if active.len() > 1 {
            return Err(format!(
                "step {index} has multiple operations: {}",
                active.join(", ")
            ));
        }
        let operation = TimelineOperation::parse(active[0])
            .expect("active timeline operation must be in TimelineOperation::ALL");
        let allowed = operation.allowed_fields();
        for key in object.keys() {
            if key != operation.name() && !allowed.contains(&key.as_str()) {
                return Err(format!("step {index} has unknown field `{key}`"));
            }
        }
    }
    Ok(steps.len())
}

#[cfg(feature = "rhai")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TimelineOperation {
    MoveTo,
    MoveToEntity,
    Possess,
    Brake,
    Command,
    Emit,
    Wait,
    WaitEvent,
}

#[cfg(feature = "rhai")]
impl TimelineOperation {
    const ALL: &[Self] = &[
        Self::MoveTo,
        Self::MoveToEntity,
        Self::Possess,
        Self::Brake,
        Self::Command,
        Self::Emit,
        Self::Wait,
        Self::WaitEvent,
    ];

    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "move_to" => Self::MoveTo,
            "move_to_entity" => Self::MoveToEntity,
            "possess" => Self::Possess,
            "brake" => Self::Brake,
            "cmd" => Self::Command,
            "emit" => Self::Emit,
            "wait" => Self::Wait,
            "wait_event" => Self::WaitEvent,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Self::MoveTo => "move_to",
            Self::MoveToEntity => "move_to_entity",
            Self::Possess => "possess",
            Self::Brake => "brake",
            Self::Command => "cmd",
            Self::Emit => "emit",
            Self::Wait => "wait",
            Self::WaitEvent => "wait_event",
        }
    }

    fn allowed_fields(self) -> &'static [&'static str] {
        match self {
            Self::MoveTo | Self::MoveToEntity => &["speed", "radius", "subject"],
            Self::Possess | Self::Wait | Self::WaitEvent => &[],
            Self::Brake => &["secs", "subject"],
            Self::Command => &["params"],
            Self::Emit => &["value"],
        }
    }
}

/// Fixed executor source; mission data arrives as the typed `ctx` map.
#[cfg(feature = "rhai")]
const TIMELINE_EXECUTOR_SOURCE: &str = "fn task(me, ctx) { seq(compile_timeline(ctx.steps)) }\n";

#[cfg(feature = "rhai")]
#[on_command(RunTimeline)]
fn on_run_timeline(
    _t: On<RunTimeline>,
    mut registry: ResMut<ScriptRegistry>,
    q_existing: Query<&ScriptedModel>,
    guard: Option<Res<lunco_core_session::SyncApplyGuard>>,
    mut commands: Commands,
) -> Result<Ack, String> {
    let step_count = timeline_step_count(&cmd.timeline).map_err(|e| format!("RunTimeline: {e}"))?;
    let (doc_id_raw, generation) = attach_rhai_scenario(
        cmd.target,
        TIMELINE_EXECUTOR_SOURCE.to_string(),
        cmd.timeline.clone(),
        // Generated source — no file, no id, no relative imports.
        None,
        ScenarioSourceMode::Runtime,
        false,
        ScenarioReloadPolicy::Retain,
        guard.and_then(|g| g.0),
        &mut registry,
        &q_existing,
        &mut commands,
    )?;
    Ok(Ack::with_data(
        OpId::new(),
        lunco_api_core::api_value!({
            "document_id": doc_id_raw,
            "generation": generation,
            "steps": step_count,
        }),
    ))
}

/// Save a named mission **timeline** to the Twin — the storage counterpart of
/// `RunTimeline` (which runs an inline one). Validates the typed timeline,
/// stores it in the [`crate::timelines::TimelineStore`], and mirrors its
/// persistent representation to `<twin>/timelines/<name>.json` (selected on
/// mount by the active Twin's Rhai loading policy). Discover with
/// `ListTimelines`/`GetTimeline`, run with `RunStoredTimeline`. Idempotent
/// (re-registering a name replaces it).
#[cfg(feature = "rhai")]
#[Command(default)]
pub struct RegisterTimeline {
    pub name: String,
    /// Structured timeline object with required `steps` and optional `name`.
    pub timeline: ScenarioParameters,
}

#[cfg(feature = "rhai")]
#[on_command(RegisterTimeline)]
fn on_register_timeline(
    _t: On<RegisterTimeline>,
    mut store: ResMut<crate::timelines::TimelineStore>,
    // Optional: present only with the workspace plugin; used to persist to the
    // active Twin's `timelines/` dir. `None` (headless / no-twin) keeps it in-memory.
    ws: Option<Res<lunco_workspace::WorkspaceResource>>,
    // Journal handle (present once wired). Records the registration as a
    // `DomainKind::Timeline` op so it syncs + persists via the journal plane;
    // fires for LOCAL registrations only (remote ones arrive via the replay leg).
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
) -> Result<Ack, String> {
    lunco_scripting_rhai_core::names::validate_file_stem(&cmd.name)
        .map_err(|error| format!("RegisterTimeline: {error}"))?;
    // Reject malformed timelines at store time, not at run time.
    timeline_step_count(&cmd.timeline).map_err(|e| format!("RegisterTimeline: {e}"))?;
    let owner = crate::timelines::active_owner(ws.as_deref())
        .map_err(|e| format!("RegisterTimeline: cannot register without an active scope: {e}"))?;
    #[cfg(not(target_arch = "wasm32"))]
    if let crate::timelines::TimelineOwner::Twin(twin_id) = owner {
        let root = ws
            .as_ref()
            .and_then(|workspace| workspace.twin(twin_id))
            .map(|twin| twin.root.clone())
            .ok_or_else(|| "RegisterTimeline: active Twin is not registered".to_string())?;
        crate::timelines::save_timeline_file(&root, &cmd.name, &cmd.timeline)
            .map_err(|error| format!("RegisterTimeline: could not persist Twin file: {error}"))?;
    }
    store.ensure_scope(owner);
    store
        .insert_for(owner, cmd.name.clone(), cmd.timeline.clone())
        .map_err(|_| "RegisterTimeline: timeline store ownership changed".to_string())?;
    if let Some(journal) = journal.as_ref() {
        crate::registration_journal::record_timeline(journal, &cmd.name, &cmd.timeline);
    }
    Ok(Ack::with_data(
        OpId::new(),
        lunco_api_core::api_value!({ "name": cmd.name.clone(), "timelines": store.names() }),
    ))
}

/// Run a stored mission timeline on an entity by name (resolved from the
/// [`crate::timelines::TimelineStore`]) — the one-step "fetch + run" for a
/// `RegisterTimeline`d / file-authored mission, sparing callers a
/// `GetTimeline`→`RunTimeline` round-trip. Same execution path as `RunTimeline`.
#[cfg(feature = "rhai")]
#[Command]
pub struct RunStoredTimeline {
    #[authz_target]
    pub target: Entity,
    pub name: String,
}

#[cfg(feature = "rhai")]
#[on_command(RunStoredTimeline)]
fn on_run_stored_timeline(
    _t: On<RunStoredTimeline>,
    store: Res<crate::timelines::TimelineStore>,
    ws: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut registry: ResMut<ScriptRegistry>,
    q_existing: Query<&ScriptedModel>,
    guard: Option<Res<lunco_core_session::SyncApplyGuard>>,
    mut commands: Commands,
) -> Result<Ack, String> {
    let owner = crate::timelines::active_owner(ws.as_deref())
        .map_err(|e| format!("RunStoredTimeline: cannot run without an active scope: {e}"))?;
    if store.owner() != Some(owner) {
        return Err(format!(
            "RunStoredTimeline: timeline store belongs to {:?}, current scope is {:?}",
            store.owner(),
            owner
        ));
    }
    // Own the typed timeline so the store borrow is released before attaching it.
    let timeline = store
        .get(&cmd.name)
        .ok_or_else(|| format!("RunStoredTimeline: no timeline named '{}'", cmd.name))?
        .clone();
    let step_count =
        timeline_step_count(&timeline).map_err(|e| format!("RunStoredTimeline: {e}"))?;
    let (doc_id_raw, generation) = attach_rhai_scenario(
        cmd.target,
        TIMELINE_EXECUTOR_SOURCE.to_string(),
        timeline,
        // Generated source — no file, no id, no relative imports.
        None,
        ScenarioSourceMode::Runtime,
        false,
        ScenarioReloadPolicy::Retain,
        guard.and_then(|g| g.0),
        &mut registry,
        &q_existing,
        &mut commands,
    )?;
    Ok(Ack::with_data(
        OpId::new(),
        lunco_api_core::api_value!({
            "name": cmd.name.clone(),
            "document_id": doc_id_raw,
            "generation": generation,
            "steps": step_count,
        }),
    ))
}

/// Declare data-driven RBAC policies for the script-execution commands in the
/// shared [`lunco_core_session::CommandPolicyRegistry`], so script submission is
/// gated through the **same authorization seam** as every other command —
/// instead of sitting at the registry's OPEN default while only the node-role
/// [`crate::scripts_run_here`] condition (which decides *where* scripts run, not
/// *who* may submit them) guards execution.
///
/// Rationale (design §3.4 "Security"): a script body reaches the *entire*
/// `cmd()` surface and executes under host authority, so a script-executing
/// command is a privilege amplifier — a networked `Observer` that may not
/// `SetPorts` directly could otherwise submit a scenario that does. We
/// therefore declare an **`Operator`** floor for the script-executing /
/// disk-persisting commands. The generic scripting host owns the policy for
/// scenario lifecycle commands.
/// Deployments relax or tighten any of these at runtime via
/// [`lunco_core_session::CommandPolicyRegistry::set_override`] with no recompile.
///
/// Scope: only *networked client* submissions reach [`lunco_core_session::authorize`]
/// (the host gate in `lunco-networking`); local host / standalone API + MCP
/// commands never do, so single-player and host-local tooling are unaffected.
#[cfg(feature = "rhai")]
pub(crate) fn register_command_policies(app: &mut App) {
    use lunco_core_session::{AuthorityRole, CommandPolicy, CommandPolicyRegistry};

    // The registry is a `LunCoCoreSessionPlugin` resource; init defensively in case the
    // scripting plugin is added first (`init_resource` is idempotent and keeps
    // the existing instance + its baseline `SetPorts` entry).
    app.init_resource::<CommandPolicyRegistry>();
    let mut reg = app.world_mut().resource_mut::<CommandPolicyRegistry>();

    // Executes a script body (full `cmd()` reach under host authority) or
    // persists an authoring artifact to the twin dir → `Operator` floor.
    const EXEC: CommandPolicy = CommandPolicy {
        min_role: AuthorityRole::Operator,
        ownership_gated: false,
    };

    reg.register("RunRhai", EXEC);
    reg.register("RunRhaiTool", EXEC);
    reg.register("RunRhaiToolHook", EXEC);
    reg.register("ApplyScriptOps", EXEC);
    reg.register("RunScenario", EXEC);
    reg.register("RunScenarioAsset", EXEC);
    reg.register("RunTimeline", EXEC);
    reg.register("RegisterTimeline", EXEC);
    reg.register("RunStoredTimeline", EXEC);
    reg.register("RegisterToolLibrary", EXEC);

    // The structural mutation verbs (`add`/`remove`/`despawn`) restructure a
    // target entity directly via reflection rather than through a command, but
    // are gated through this SAME registry under a well-known capability key
    // (`bridge_core::capability::STRUCTURAL_MUTATE`): ownership-gated control, so
    // a remote script may only restructure entities its launching session owns.
    reg.register(
        bridge_core::capability::STRUCTURAL_MUTATE,
        CommandPolicy::OWNED_CONTROL,
    );
    reg.register(
        bridge_core::capability::FIELD_MUTATE,
        CommandPolicy::OWNED_CONTROL,
    );
    reg.register(
        bridge_core::capability::PORT_MUTATE,
        CommandPolicy::OWNED_CONTROL,
    );
    reg.register(
        bridge_core::capability::SETTING_MUTATE,
        CommandPolicy {
            min_role: AuthorityRole::Operator,
            ownership_gated: false,
        },
    );
    reg.register(
        bridge_core::capability::POLICY_MUTATE,
        CommandPolicy {
            min_role: AuthorityRole::Operator,
            ownership_gated: false,
        },
    );
}

// Generates `register_all_commands` for the compiled-in Rhai commands.
register_commands!(
    on_apply_script_ops,
    on_run_rhai,
    on_run_rhai_tool,
    on_run_rhai_tool_hook,
    on_run_scenario,
    on_run_scenario_asset,
    on_run_timeline,
    on_register_timeline,
    on_run_stored_timeline,
    on_register_tool_library,
);

#[cfg(all(test, feature = "rhai"))]
mod tests {
    //! Runtime behavior for typed timeline values is covered by the authored
    //! Rhai scene-test gate; this module checks only the generic reflection seam.

    #[test]
    fn run_timeline_reflection_accepts_typed_steps_map() {
        use bevy::prelude::{App, AppTypeRegistry};
        use lunco_api_core::ApiValue;

        let mut app = App::new();
        super::__register_on_run_timeline(&mut app);
        let target = app.world_mut().spawn_empty().id();
        let registry = app.world().resource::<AppTypeRegistry>().read();
        let registration = registry
            .get_with_short_type_path("RunTimeline")
            .expect("RunTimeline is reflected");
        let params = ApiValue::map([
            ("target", ApiValue::Int(target.to_bits() as i64)),
            (
                "timeline",
                ApiValue::map([(
                    "steps",
                    ApiValue::Array(vec![ApiValue::map([("wait", ApiValue::Float(0.0))])]),
                )]),
            ),
        ]);

        lunco_api::executor::validate_command_params_value(
            "RunTimeline",
            &params,
            registration,
            &registry,
            &lunco_api::ApiEntityRegistry::default(),
        )
        .expect("typed structured timeline parameters deserialize");
    }

    #[test]
    fn run_scenario_asset_reflection_accepts_omitted_host() {
        use bevy::prelude::{App, AppTypeRegistry};
        use lunco_api_core::api_value;

        let mut app = App::new();
        super::__register_on_run_scenario_asset(&mut app);
        let registry = app.world().resource::<AppTypeRegistry>().read();
        let registration = registry
            .get_with_short_type_path("RunScenarioAsset")
            .expect("RunScenarioAsset is reflected");
        let params = api_value!({
            "source_asset": "lunco://tutorials/sandbox/first_drive.rhai",
            "scene_asset": "lunco://tutorials/sandbox/first_drive.usda",
            "reload_policy": "Restart"
        });

        lunco_api::executor::validate_command_params_value(
            "RunScenarioAsset",
            &params,
            registration,
            &registry,
            &lunco_api::ApiEntityRegistry::default(),
        )
        .expect("tutorial launch may omit its default WorldRoot host");
    }

    #[test]
    fn script_commands_carry_rbac_policies() {
        use bevy::prelude::*;
        use lunco_core_session::{AuthorityRole, CommandPolicy, CommandPolicyRegistry};

        let mut app = App::new();
        super::register_command_policies(&mut app);
        let reg = app.world().resource::<CommandPolicyRegistry>();

        // Script-executing / disk-persisting commands carry an Operator floor:
        // a body reaches the whole cmd() surface under host authority.
        let exec = CommandPolicy {
            min_role: AuthorityRole::Operator,
            ownership_gated: false,
        };
        for c in [
            "RunRhai",
            "RunRhaiTool",
            "RunScenario",
            "RunTimeline",
            "RegisterTimeline",
            "RunStoredTimeline",
            "RegisterToolLibrary",
        ] {
            assert_eq!(reg.policy_for(c), exec, "{c} should require Operator");
        }

        // The structural mutation verbs share the registry under a capability key,
        // ownership-gated so a remote script only restructures what it owns.
        assert_eq!(
            reg.policy_for(super::bridge_core::capability::STRUCTURAL_MUTATE),
            CommandPolicy::OWNED_CONTROL,
        );

        // The baseline core entries survive our defensive init_resource.
        assert_eq!(reg.policy_for("SetPorts"), CommandPolicy::OWNED_CONTROL);
        // An undeclared command stays OPEN (the RBAC-readiness invariant).
        assert_eq!(reg.policy_for("SomeUngatedQuery"), CommandPolicy::OPEN);
    }
}
