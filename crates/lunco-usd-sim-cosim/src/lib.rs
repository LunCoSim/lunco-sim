//! USD → cosim translator.
//!
//! Reads `lunco:modelicaModel` / `lunco:pythonModel` and native
//! `connectionPaths` from USD prims after the bounded USD visual projection
//! has spawned the entity, and drives the full cosim lifecycle end-to-end:
//!
//! - **Modelica**: opens the source file, inserts a `ModelicaModel`
//!   stub, dispatches `ModelicaCommand::Compile` directly to the
//!   worker channel, and publishes the `SimComponent` — the entity's
//!   port interface — immediately from the parsed declaration, so wires
//!   into the model resolve before the solver has answered
//!   (`SimStatus::Compiling` until `model.variables` populates).
//! - **Python**: opens the script, registers a `ScriptDocument`,
//!   attaches `ScriptedModel`, and creates the matching `SimComponent`.
//! - **Wiring**: [`rewire_usd_connections`] derives one `SimConnection`
//!   per authored `connectionPaths` source on a prim's `inputs:*`
//!   attributes — a consuming input `/B.inputs:force_y` connected to a
//!   producing output `/A.outputs:netForce` (self-loop when `A == B`).
//!   The derived set is a pure cache of USD, rebuilt on stage change.
//!
//! No domain-specific ECS marker is inserted here. The translator is the
//! authoritative path for USD-defined cosim entities.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use lunco_core::{DiagnosticSeverity, RuntimeDiagnostic, RuntimeDiagnostics};
use lunco_cosim_core::{
    BindingEpochDirty, ConnectionBinding, DeclaredOutputPorts, SimComponent, SimConnection,
    SimStatus, UsdSourcedCosim,
};
#[cfg(feature = "python")]
use lunco_doc::{DocumentId, DocumentOrigin};
use lunco_modelica_runtime::source_asset::ModelicaSource;
use lunco_modelica_runtime::{
    ModelicaChannels, ModelicaCommand, ModelicaModel, ModelicaSignalLayout,
};
#[cfg(feature = "python")]
use lunco_scripting::doc::{ScriptDocument, ScriptLanguage};
#[cfg(feature = "python")]
use lunco_scripting::python::{PythonStatus, get_python_status};
#[cfg(feature = "python")]
use lunco_scripting::source_asset::PythonSource;
#[cfg(feature = "python")]
use lunco_scripting::{SceneOwnedScript, ScriptRegistry, doc::ScriptedModel};
use lunco_telemetry_core::{ChannelSource, Parameter};
use lunco_usd_bevy_runtime_core::scene::SceneLoadInFlight;
use lunco_usd_bevy_scene::{UsdPreviewOnly, UsdPrimPath};
use lunco_usd_bevy_stage::read::UsdReadObject;
use lunco_usd_bevy_stage::read::read_authored_bool_strict;
use lunco_usd_bevy_stage::{
    UsdInstanceProjection, UsdInstanceRoot, UsdStageAsset, UsdWiringDirty,
    canonical::CanonicalStages,
};
use openusd::sdf::{Path as SdfPath, Value};
use std::collections::{BTreeSet, HashMap};

use lunco_usd_sim_core::{PendingDifferential, PendingEntityWork, UsdSimProcessed, UsdSimSet};
use lunco_usd_sim_domain::{GeneratedModelicaSource, UsdModelicaPortContract, UsdModelicaSchedule};

/// Installs USD-authored co-simulation participant and connection projection.
pub struct UsdSimCosimPlugin;

pub mod readiness;
pub mod sync;
mod wiring;

pub use wiring::{
    BindingEpochWait, UsdWiredConnection, install_wiring_system, modelica_models_terminal,
};
use wiring::{
    BindingModelStatuses, WiringFactsCache, causal_participants_changed,
    derive_causal_barrier_participants, forget_binding_model_status,
    install_wiring_invalidation_observers, request_binding_epoch,
    request_binding_epoch_on_model_change, request_binding_epoch_on_remove,
    reset_wiring_facts_cache, rewire_usd_connections, settle_binding_epoch, wiring_due,
};

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum CosimUpdateSet {
    Scene,
    Projection,
    Wiring,
}

/// Marks a USD prim after its authored telemetry declaration has been projected
/// into the runtime sampling plan. The marker is scene-lifetime state: a scene
/// reload despawns the prim and therefore naturally re-projects its channels.
#[derive(Component)]
struct UsdTelemetryProjected;

/// Runtime channels are projection output, not scene identity. This marker
/// lets a composed-stage revision remove stale sampling channels before the
/// declarations are projected again.
#[derive(Component)]
struct UsdTelemetryChannel;

/// Runtime index for the one-time USD telemetry projection.
///
/// `dirty` admits a projection pass; `invalidation_pending` is the coalesced
/// lifecycle signal that makes the index and its output channels stale. The
/// first pass is also the one-time discovery for entities that predate this
/// plugin. Normal invalidation is fed by component lifecycle observers rather
/// than population queries in Update.
#[derive(Resource)]
struct UsdTelemetryProjectionIndex {
    generated_outputs: HashMap<
        (
            bevy::asset::AssetId<UsdStageAsset>,
            Option<u64>,
            String,
            String,
        ),
        (Entity, String),
    >,
    entities_by_path: HashMap<(bevy::asset::AssetId<UsdStageAsset>, String), Entity>,
    generated_entities_by_path: HashMap<(bevy::asset::AssetId<UsdStageAsset>, String), Entity>,
    diagnostics: HashMap<(bevy::asset::AssetId<UsdStageAsset>, String), RuntimeDiagnostic>,
    observed_stage_revision: u64,
    invalidation_pending: bool,
    dirty: bool,
}

impl Default for UsdTelemetryProjectionIndex {
    fn default() -> Self {
        Self {
            generated_outputs: HashMap::new(),
            entities_by_path: HashMap::new(),
            generated_entities_by_path: HashMap::new(),
            diagnostics: HashMap::new(),
            observed_stage_revision: 0,
            invalidation_pending: false,
            dirty: true,
        }
    }
}

fn invalidate_usd_telemetry_projection_index_on_insert<T: Component>(
    _: On<Insert, T>,
    mut index: ResMut<UsdTelemetryProjectionIndex>,
) {
    index.invalidation_pending = true;
}

fn invalidate_usd_telemetry_projection_index_on_remove<T: Component>(
    _: On<Remove, T>,
    mut index: ResMut<UsdTelemetryProjectionIndex>,
) {
    index.invalidation_pending = true;
}

fn mark_usd_telemetry_projection_index_dirty(
    mut index: ResMut<UsdTelemetryProjectionIndex>,
    stage_revision: Option<Res<lunco_usd_bevy_scene::UsdStageRevision>>,
    projected: Query<Entity, With<UsdTelemetryProjected>>,
    channels: Query<Entity, With<UsdTelemetryChannel>>,
    stage_assets: Option<Res<Assets<UsdStageAsset>>>,
    mut commands: Commands,
) {
    let revision_changed = stage_revision
        .as_ref()
        .is_some_and(|revision| revision.0 != index.observed_stage_revision);
    let stage_assets_changed = stage_assets
        .as_ref()
        .is_some_and(|assets| assets.is_changed());
    let lifecycle_changed = std::mem::take(&mut index.invalidation_pending);
    if !lifecycle_changed && !revision_changed && !stage_assets_changed {
        return;
    }

    index.dirty = true;
    index.diagnostics.clear();
    if let Some(revision) = stage_revision {
        index.observed_stage_revision = revision.0;
    }
    for entity in &projected {
        commands.entity(entity).remove::<UsdTelemetryProjected>();
    }
    for entity in &channels {
        commands.entity(entity).try_despawn();
    }
}

fn telemetry_projection_index_invalidation_due(
    index: Res<UsdTelemetryProjectionIndex>,
    stage_revision: Option<Res<lunco_usd_bevy_scene::UsdStageRevision>>,
    stage_assets: Option<Res<Assets<UsdStageAsset>>>,
) -> bool {
    index.invalidation_pending
        || stage_revision
            .as_ref()
            .is_some_and(|revision| revision.0 != index.observed_stage_revision)
        || stage_assets
            .as_ref()
            .is_some_and(|assets| assets.is_changed())
}

fn telemetry_projection_needed(index: Res<UsdTelemetryProjectionIndex>) -> bool {
    index.dirty
}

fn reset_usd_telemetry_projection_index(mut index: ResMut<UsdTelemetryProjectionIndex>) {
    index.generated_outputs.clear();
    index.entities_by_path.clear();
    index.generated_entities_by_path.clear();
    index.diagnostics.clear();
    index.observed_stage_revision = 0;
    index.invalidation_pending = false;
    index.dirty = true;
}

/// Telemetry event published when a USD-declared model could not be handed to
/// the solver at all — the worker channel was closed, so the compile that
/// `SimStatus::Compiling` is waiting for will never be attempted.
///
/// Published at [`lunco_telemetry_core::Severity::Error`] so the workbench status bar's
/// error-telemetry observer surfaces it. A scene whose models silently
/// never step is indistinguishable from a scene that is merely still compiling;
/// the difference has to reach the UI, not just the log.
pub const MODEL_DISPATCH_FAILED: &str = "MODEL_DISPATCH_FAILED";

/// Telemetry event for a USD program whose authored execution policy is
/// invalid. It is distinct from a worker dispatch failure: the source was
/// never admitted because the scene configuration itself is not executable.
pub const MODEL_CONFIGURATION_INVALID: &str = "MODEL_CONFIGURATION_INVALID";

/// Scene-scoped diagnostics for Python programs that are authored in USD but
/// cannot run in this binary. The prim itself carries the durable `Error`
/// status; this resource only collects the paths so startup can report one
/// actionable scene-level verdict instead of forcing a tester to find each
/// per-prim warning in a long load log.
#[derive(Resource, Default, Debug)]
pub(crate) struct PythonUnavailablePrograms {
    paths: BTreeSet<String>,
    reported: bool,
}

/// A prim's USD-declared co-sim interface — its `inputs:`/`outputs:` scalar
/// attributes — as value maps seeded at zero.
///
/// USD is the public contract, so this is what every wire resolves against. It is
/// read at BIND time and published into [`SimComponent`] immediately — before the
/// async source load or compile — for EVERY participant kind (Modelica and Python
/// alike). Publishing at bind removes the window in which a wire into a declared
/// port would transiently read as an unknown input; the one shared extraction
/// keeps the two participant paths from drifting (Python used to ship an empty
/// interface, so every wire into it false-warned until — or unless — the port
/// happened to be claimed by another backend).
fn declared_port_name(attr: &str, namespace: &str) -> Option<String> {
    attr.strip_prefix(namespace)
        .map(|name| name.strip_suffix(".connect").unwrap_or(name).to_owned())
}

fn declared_interface(
    reader: &dyn UsdReadObject,
    sdf_path: &SdfPath,
) -> (HashMap<String, f64>, HashMap<String, f64>) {
    let mut inputs = HashMap::new();
    let mut outputs = HashMap::new();
    for attr in reader.attr_names(sdf_path) {
        if let Some(name) = declared_port_name(&attr, "inputs:") {
            inputs.insert(name, 0.0);
        } else if let Some(name) = declared_port_name(&attr, "outputs:") {
            outputs.insert(name, 0.0);
        }
    }
    (inputs, outputs)
}

/// A prim that is both a Modelica program and a rigid body has one authored
/// `inputs:*` namespace, but the standard Avian mass/force ports are physical
/// sinks, not Modelica solver inputs. Keep those names out of the map-backed
/// Modelica interface so the Avian backend remains the single writer. A
/// controller that needs a physical value uses an explicitly distinct input
/// (for example `controller_inertia_xx`) and a second USD connection to the
/// same source; there is no backend-precedence alias.
fn strip_rigid_body_inputs(
    reader: &dyn UsdReadObject,
    sdf_path: &SdfPath,
    inputs: &mut HashMap<String, f64>,
) {
    if !reader.has_api_schema(sdf_path, "PhysicsRigidBodyAPI") {
        return;
    }
    for name in [
        "force_x",
        "force_y",
        "force_z",
        "force_local_x",
        "force_local_y",
        "force_local_z",
        "torque_x",
        "torque_y",
        "torque_z",
        "mass",
        "inertia_xx",
        "inertia_yy",
        "inertia_zz",
        "com_x",
        "com_y",
        "com_z",
    ] {
        inputs.remove(name);
    }
}

/// Publish the statically declared scalar interface of an environment probe.
///
/// `LunCoEnvironmentProbeAPI` declares gravity outputs on the schema class. The
/// live OpenUSD `property_names()` query intentionally reports authored
/// properties, not properties inherited from a codeless API schema, so using
/// [`declared_interface`] alone would create an empty source component for the
/// usual empty `probe.usda` asset. The environment domain owns the gravity
/// outputs; direction outputs are materialized from composed wire demand by
/// the generic direction publisher.
fn environment_probe_interface() -> DeclaredOutputPorts {
    DeclaredOutputPorts {
        names: lunco_cosim_core::ENVIRONMENT_PROBE_BASE_OUTPUTS
            .iter()
            .map(|name| (*name).to_owned())
            .collect(),
    }
}

/// A compile-specific port-contract verdict already reported to the console.
///
/// Keeping the session id makes validation reactive to a later recompile while
/// ensuring an unchanged bad model produces one actionable diagnostic, not one
/// per fixed tick.
#[derive(Component)]
struct ValidatedUsdModelicaPortContract {
    session_id: u64,
}

/// Queued Modelica source load. Inserted by `process_usd_cosim_prims`;
/// drained by `dispatch_loaded_modelica_sources` once the
/// `Handle<ModelicaSource>` has resolved to bytes.
#[derive(Component)]
pub struct PendingModelicaSource {
    pub handle: Handle<ModelicaSource>,
    /// Asset-relative path, copied into the generated source's stable compiler
    /// URI and diagnostics metadata.
    pub asset_path: String,
    /// Modelica worker session for this source load. Recompile requests carry
    /// a newer session so results from the superseded solver are fenced.
    pub session_id: u64,
    /// Whether a successful compile should resume this live participant.
    pub resume_after_compile: bool,
}

/// Same for Python.
#[derive(Component)]
#[cfg(feature = "python")]
pub struct PendingPythonSource {
    pub handle: Handle<PythonSource>,
    pub asset_path: String,
}

/// Coalesced USD-prim discovery work for the cosimulation projection.
#[derive(Resource)]
struct PendingUsdCosimPrimWork(PendingEntityWork);

impl Default for PendingUsdCosimPrimWork {
    fn default() -> Self {
        Self(PendingEntityWork::with_initial_discovery())
    }
}

/// Lifecycle-queued Modelica owners that still need their shared port surface.
#[derive(Resource)]
struct PendingModelicaWrapWork(PendingEntityWork);

impl Default for PendingModelicaWrapWork {
    fn default() -> Self {
        Self(PendingEntityWork::with_initial_discovery())
    }
}

fn queue_added_usd_cosim_prim(
    trigger: On<Add, UsdPrimPath>,
    unprocessed: Query<(), Without<UsdSourcedCosim>>,
    mut pending: ResMut<PendingUsdCosimPrimWork>,
) {
    if unprocessed.contains(trigger.entity) {
        pending.0.queue(trigger.entity);
    }
}

fn forget_removed_usd_cosim_prim(
    trigger: On<Remove, UsdPrimPath>,
    mut pending: ResMut<PendingUsdCosimPrimWork>,
) {
    pending.0.forget(trigger.entity);
}

fn queue_removed_usd_sourced_cosim(
    trigger: On<Remove, UsdSourcedCosim>,
    prims: Query<(), With<UsdPrimPath>>,
    mut pending: ResMut<PendingUsdCosimPrimWork>,
) {
    if prims.contains(trigger.entity) {
        pending.0.queue(trigger.entity);
    }
}

fn queue_modelica_wrap_for_new_model(
    trigger: On<Add, ModelicaModel>,
    eligible: Query<(), (With<UsdSourcedCosim>, Without<SimComponent>)>,
    mut pending: ResMut<PendingModelicaWrapWork>,
) {
    if eligible.contains(trigger.entity) {
        pending.0.queue(trigger.entity);
    }
}

fn queue_modelica_wrap_for_new_cosim_owner(
    trigger: On<Add, UsdSourcedCosim>,
    eligible: Query<(), (With<ModelicaModel>, Without<SimComponent>)>,
    mut pending: ResMut<PendingModelicaWrapWork>,
) {
    if eligible.contains(trigger.entity) {
        pending.0.queue(trigger.entity);
    }
}

fn queue_modelica_wrap_after_surface_removal(
    trigger: On<Remove, SimComponent>,
    eligible: Query<(), (With<UsdSourcedCosim>, With<ModelicaModel>)>,
    mut pending: ResMut<PendingModelicaWrapWork>,
) {
    if eligible.contains(trigger.entity) {
        pending.0.queue(trigger.entity);
    }
}

fn forget_removed_modelica_wrap_source(
    trigger: On<Remove, ModelicaModel>,
    mut pending: ResMut<PendingModelicaWrapWork>,
) {
    pending.0.forget(trigger.entity);
}

fn forget_removed_modelica_wrap_owner(
    trigger: On<Remove, UsdSourcedCosim>,
    mut pending: ResMut<PendingModelicaWrapWork>,
) {
    pending.0.forget(trigger.entity);
}

fn reset_usd_cosim_prim_work(mut pending: ResMut<PendingUsdCosimPrimWork>) {
    pending.0.clear();
}

fn reset_modelica_wrap_work(mut pending: ResMut<PendingModelicaWrapWork>) {
    pending.0.clear();
}

/// Reads cosim attributes from USD prims and dispatches model
/// compilation + wires. Runs in `Update` after `sync_usd_visuals` so
/// `Transform` / `Mesh3d` / `Material` are already present.
/// Run condition: the initial discovery or a queued prim lifecycle is pending.
fn any_unprocessed_usd_cosim(pending: Res<PendingUsdCosimPrimWork>) -> bool {
    pending.0.has_work()
}

/// Run condition: any `UsdSourcedCosim` modelica model still needs wrapping
/// into a `SimComponent`.
fn any_pending_modelica_wrap(pending: Res<PendingModelicaWrapWork>) -> bool {
    pending.0.has_work()
}

pub(crate) fn process_usd_cosim_prims(
    mut commands: Commands,
    query: Query<(Entity, &UsdPrimPath, Option<&UsdInstanceProjection>), Without<UsdSourcedCosim>>,
    mut pending: ResMut<PendingUsdCosimPrimWork>,
    parents: Query<&ChildOf>,
    preview_roots: Query<(), With<UsdPreviewOnly>>,
    stages: Res<Assets<UsdStageAsset>>,
    // Initial reads use the worker-produced plan; later authored generations
    // use the live canonical stage selected by the shared reader boundary.
    canonical: NonSend<CanonicalStages>,
    asset_server: Res<AssetServer>,
    mut wiring_dirty: ResMut<UsdWiringDirty>,
    mut python_unavailable: ResMut<PythonUnavailablePrograms>,
) {
    // Which prims a component collection already owns, per stage. Computed once
    // per batch rather than per prim.
    let mut members_by_stage: HashMap<bevy::asset::AssetId<UsdStageAsset>, BTreeSet<String>> =
        HashMap::new();
    let mut entities = pending.0.take_queued();
    if pending.0.take_initial_discovery() {
        // This single bootstrap pass covers entities that predate plugin
        // installation. Normal scene arrivals are queued by the lifecycle
        // observer and do not need a population scan.
        entities.extend(query.iter().map(|(entity, _, _)| entity));
    }
    let mut entities: Vec<_> = entities.into_iter().collect();
    entities.sort_unstable();
    for entity in entities {
        let Ok((entity, prim_path, instance_projection)) = query.get(entity) else {
            continue;
        };
        if lunco_usd_bevy_scene::is_preview_only(entity, &parents, &preview_roots) {
            continue;
        }
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else {
            pending.0.queue(entity);
            continue;
        };

        let id = prim_path.stage_handle.id();
        // Record that this prim's authored cosim surface has been examined,
        // including non-programmable prims, before any early return below.
        // Other cosim consumers also require a model/script participant, so
        // this ownership marker alone does not make a visual prim a solver.
        let Some(stage_asset) = stages.get(&prim_path.stage_handle) else {
            pending.0.queue(entity);
            continue;
        };
        let (reader, _generation) =
            canonical.reader_for_entity(id, stage_asset, instance_projection);
        // `try_insert` (not `.insert`): a `LoadScene` cleanup may despawn this
        // prim between this system's iterate and ApplyDeferred — the canonical
        // race is the moonbase autoload vs a first-run tutorial on web. `.insert`
        // routes through Bevy's panic error handler, which aborts wasm; `try_insert`
        // silently drops the write on a despawned entity. Every entity-tied insert
        // queued by this pipeline uses the same despawn-safe form for the same
        // reason. The visual synchronization boundary owns that policy.
        commands.entity(entity).try_insert(UsdSourcedCosim);
        if reader.has_api_schema(&sdf_path, "LunCoDirectionTargetAPI")
            && !reader.has_api_schema(&sdf_path, "LunCoCelestialBodyAPI")
        {
            if let Some(target_id) = reader
                .text(&sdf_path, "lunco:directionTarget:id")
                .and_then(|value| lunco_environment::DirectionTargetId::new(&value))
            {
                commands.entity(entity).try_insert(target_id);
            }
        }
        if reader.has_api_schema(&sdf_path, "LunCoEnvironmentProbeAPI") {
            let declared_outputs = environment_probe_interface();
            commands.entity(entity).try_insert((
                lunco_environment::EnvironmentProbe,
                SimComponent {
                    model_name: "EnvironmentProbe".into(),
                    ..default()
                },
                declared_outputs,
            ));
            // A stage may finish composing after the prim's Added event. Force
            // the native USD wiring cache to resolve connections from this
            // newly published source interface in the same update cycle.
            wiring_dirty.0 = true;
            continue;
        }
        let members = members_by_stage.entry(id).or_insert_with(|| {
            lunco_usd_bevy_core::program::modelica_network_member_paths(&reader)
                .into_iter()
                .collect()
        });
        process_usd_cosim_prim_read(
            &reader,
            entity,
            prim_path,
            &sdf_path,
            members,
            &mut commands,
            &asset_server,
            &mut wiring_dirty,
            &mut python_unavailable,
        );
    }
}

/// Report Python availability once the scene's USD prims have finished
/// materialising. This is deliberately separate from the per-prim bind
/// diagnostic: the bind owns the precise error and durable component state,
/// while this system gives the scene author one concise verdict.
fn report_python_unavailable(
    mut diagnostics: ResMut<PythonUnavailablePrograms>,
    in_flight: Option<Res<SceneLoadInFlight>>,
    pending: Res<PendingUsdCosimPrimWork>,
) {
    if diagnostics.reported
        || diagnostics.paths.is_empty()
        || in_flight.is_some()
        || pending.0.has_work()
    {
        return;
    }

    diagnostics.reported = true;
    let count = diagnostics.paths.len();
    let examples = diagnostics
        .paths
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    warn!(
        "[usd-cosim] {count} Python program(s) in this scene are inert: the Python runtime is unavailable; affected prims: {examples}"
    );
}

fn reset_python_unavailable(mut diagnostics: ResMut<PythonUnavailablePrograms>) {
    *diagnostics = PythonUnavailablePrograms::default();
}

fn read_authored_telemetry_string(
    view: &dyn UsdReadObject,
    path: &SdfPath,
    attribute: &str,
) -> Result<Option<String>, ()> {
    if !view.has_authored_attribute(path, attribute) {
        return Ok(None);
    }
    // USD `token` and `string` are distinct Value variants, but both are
    // textual authored telemetry metadata.  StageView::text is the shared
    // reader for that contract; matching Value::String here silently rejected
    // every schema-declared token such as lunco:telemetry:port.
    view.text(path, attribute).map(Some).ok_or(())
}

fn read_authored_telemetry_real(
    view: &dyn UsdReadObject,
    path: &SdfPath,
    attribute: &str,
) -> Result<Option<f64>, ()> {
    if !view.has_authored_attribute(path, attribute) {
        return Ok(None);
    }
    match view.real(path, attribute) {
        Some(value) if value.is_finite() => Ok(Some(value)),
        _ => Err(()),
    }
}

/// Project the standard LunCo telemetry declaration attributes into the shared
/// telemetry sampler. The declaration stays in USD; this is only the runtime
/// projection, so descriptions and units remain authored data all the way to
/// the signal registry.
fn project_usd_telemetry(
    mut commands: Commands,
    entity_query: Query<(Entity, &UsdPrimPath, Option<&GeneratedModelicaSource>)>,
    generated_query: Query<(
        Entity,
        &UsdPrimPath,
        &GeneratedModelicaSource,
        Option<&ModelicaSignalLayout>,
        Option<&lunco_core::Provenance>,
        Option<&lunco_core::GlobalEntityId>,
        Has<UsdInstanceRoot>,
    )>,
    target_surface_query: Query<(
        Has<SimComponent>,
        Has<lunco_port_core::PortSurfaceReady>,
        Has<lunco_port_core::PortSurfacePending>,
    )>,
    pending_interface_query: Query<(), (With<UsdSourcedCosim>, Without<SimComponent>)>,
    pending_query: Query<
        (
            Entity,
            &UsdPrimPath,
            Option<&lunco_core::Provenance>,
            Option<&lunco_core::GlobalEntityId>,
            Has<UsdInstanceRoot>,
            Option<&UsdInstanceProjection>,
        ),
        Without<UsdTelemetryProjected>,
    >,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
    mut index: ResMut<UsdTelemetryProjectionIndex>,
    diagnostics: Option<ResMut<RuntimeDiagnostics>>,
) {
    // The generated wrapper is the only runtime Modelica participant.  Build
    // the authored-member -> wrapper port map from its projection metadata
    // before projecting declarations, so a member's USD telemetry never
    // creates a second, transform-only channel on the member entity.
    let instance_of = |provenance: Option<&lunco_core::Provenance>,
                       gid: Option<&lunco_core::GlobalEntityId>,
                       is_root: bool| {
        match provenance {
            Some(lunco_core::Provenance::Derived { parent, .. }) => Some(*parent),
            _ if is_root => gid.map(lunco_core::GlobalEntityId::get),
            _ => None,
        }
    };
    if index.dirty {
        index.diagnostics.clear();
        index.generated_outputs.clear();
        index.entities_by_path.clear();
        index.generated_entities_by_path.clear();
        for (entity, prim_path, generated) in &entity_query {
            let key = (prim_path.stage_handle.id(), prim_path.path.clone());
            index.entities_by_path.entry(key.clone()).or_insert(entity);
            if generated.is_some() {
                index.generated_entities_by_path.insert(key, entity);
            }
        }
        for (wrapper, prim_path, generated, layout, provenance, gid, is_root) in &generated_query {
            let Some(layout) = layout else {
                continue;
            };
            let instance = instance_of(provenance, gid, is_root);
            for (member, output, alias) in &generated.member_output_aliases {
                // A boundary output is the canonical runtime address when one
                // exists; otherwise the generated member alias is the public
                // wrapper port.  The same layout used by Modelica telemetry owns
                // this choice, so authored telemetry and solver retention cannot
                // disagree about which value they read.
                let runtime_port = layout
                    .exact_provenance
                    .get(alias)
                    .and_then(|identity| identity.canonical_name.clone())
                    .unwrap_or_else(|| alias.clone());
                index.generated_outputs.insert(
                    (
                        prim_path.stage_handle.id(),
                        instance,
                        member.clone(),
                        output.clone(),
                    ),
                    (wrapper, runtime_port),
                );
            }
        }
        index.dirty = false;
    }

    for (entity, prim_path, provenance, gid, is_root, instance_projection) in &pending_query {
        let Some(stage_asset) = stages.get(&prim_path.stage_handle) else {
            continue;
        };
        let id = prim_path.stage_handle.id();
        let (reader, _generation) =
            canonical.reader_for_entity(id, stage_asset, instance_projection);
        let Ok(path) = SdfPath::new(&prim_path.path) else {
            index.diagnostics.insert(
                (id, prim_path.path.clone()),
                RuntimeDiagnostic {
                    code: "telemetry-path".to_string(),
                    severity: DiagnosticSeverity::Error,
                    producer: "usd-telemetry".to_string(),
                    subject: prim_path.path.clone(),
                    message: "telemetry declaration has an invalid USD prim path".to_string(),
                },
            );
            commands.entity(entity).try_insert(UsdTelemetryProjected);
            continue;
        };
        let authored = match read_authored_bool_strict(&reader, &path, "lunco:telemetry") {
            Ok(Some(value)) => value,
            Ok(None) => false,
            Err(_) => {
                index.diagnostics.insert(
                    (id, path.as_str().to_owned()),
                    RuntimeDiagnostic {
                        code: "telemetry-contract".to_string(),
                        severity: DiagnosticSeverity::Error,
                        producer: "usd-telemetry".to_string(),
                        subject: path.as_str().to_owned(),
                        message:
                            "lunco:telemetry must be a boolean authored on the declaration prim"
                                .to_string(),
                    },
                );
                warn!(
                    "[usd-cosim] {} has malformed `lunco:telemetry`; declaration ignored",
                    path.as_str()
                );
                false
            }
        };
        if authored {
            let target_paths = reader.rel_targets(&path, "lunco:telemetry:target");
            let target_path = match target_paths.as_slice() {
                [] => {
                    let direct_surface = target_surface_query
                        .get(entity)
                        .is_ok_and(|(sim, ready, pending)| !pending && (sim || ready));
                    if direct_surface {
                        // The declaration prim is its own target only when it
                        // has published a runtime surface. A declaration
                        // Scope without a surface must name the measured prim
                        // explicitly; otherwise it would silently bind to the
                        // metadata Scope instead of the physical signal owner.
                        prim_path.path.clone()
                    } else {
                        index.diagnostics.insert(
                            (id, path.as_str().to_owned()),
                            RuntimeDiagnostic {
                                code: "telemetry-target".to_string(),
                                severity: DiagnosticSeverity::Error,
                                producer: "usd-telemetry".to_string(),
                                subject: path.as_str().to_owned(),
                                message: "telemetry declaration has no target relationship and its prim has no runtime port surface; author exactly one lunco:telemetry:target or place the declaration on the measured prim".to_string(),
                            },
                        );
                        commands.entity(entity).try_insert(UsdTelemetryProjected);
                        continue;
                    }
                }
                [target] => target.as_str().to_owned(),
                _ => {
                    index.diagnostics.insert(
                        (id, path.as_str().to_owned()),
                        RuntimeDiagnostic {
                            code: "telemetry-target".to_string(),
                            severity: DiagnosticSeverity::Error,
                            producer: "usd-telemetry".to_string(),
                            subject: path.as_str().to_owned(),
                            message: "telemetry declaration has multiple target relationships; author exactly one lunco:telemetry:target".to_string(),
                        },
                    );
                    warn!(
                        "[usd-cosim] {} has multiple telemetry targets; exactly one is allowed",
                        path.as_str()
                    );
                    String::new()
                }
            };
            if target_path.is_empty() {
                commands.entity(entity).try_insert(UsdTelemetryProjected);
                continue;
            }
            let target_key = (id, target_path.clone());
            let Some(target_entity) = index
                .generated_entities_by_path
                .get(&target_key)
                .copied()
                .or_else(|| index.entities_by_path.get(&target_key).copied())
            else {
                index.diagnostics.insert(
                    (id, path.as_str().to_owned()),
                    RuntimeDiagnostic {
                        code: "telemetry-target".to_string(),
                        severity: DiagnosticSeverity::Error,
                        producer: "usd-telemetry".to_string(),
                        subject: path.as_str().to_owned(),
                        message: format!(
                            "telemetry target `{target_path}` has no projected runtime entity"
                        ),
                    },
                );
                commands.entity(entity).try_insert(UsdTelemetryProjected);
                continue;
            };
            let declaration = (|| {
                let port = read_authored_telemetry_string(&reader, &path, "lunco:telemetry:port")?
                    .filter(|value| !value.is_empty());
                let reflect =
                    read_authored_telemetry_string(&reader, &path, "lunco:telemetry:reflect")?
                        .filter(|value| !value.is_empty());
                let source = match (port, reflect) {
                    (Some(port), _) => ChannelSource::Port(port),
                    (None, Some(reflect)) => ChannelSource::Reflect(reflect),
                    (None, None) => return Err(()),
                };
                let source_name = match &source {
                    ChannelSource::Port(name) => name.rsplit('.').next().unwrap_or(name.as_str()),
                    ChannelSource::Reflect(path) => {
                        path.rsplit('.').next().unwrap_or(path.as_str())
                    }
                    ChannelSource::Diagnostic(path) => path,
                };
                let name = read_authored_telemetry_string(&reader, &path, "lunco:telemetry:name")?
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| source_name.to_string());
                let display_name = reader
                    .text(&path, "ui:displayName")
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| value.trim().to_owned());
                let unit = read_authored_telemetry_string(&reader, &path, "lunco:telemetry:unit")?
                    .unwrap_or_default();
                let description =
                    read_authored_telemetry_string(&reader, &path, "lunco:telemetry:description")?;
                let rate_hz =
                    match read_authored_telemetry_real(&reader, &path, "lunco:telemetry:rateHz")? {
                        None | Some(0.0) => None,
                        Some(value) if value > 0.0 => Some(value),
                        Some(_) => return Err(()),
                    };
                let enabled =
                    match read_authored_bool_strict(&reader, &path, "lunco:telemetry:enabled") {
                        Ok(Some(value)) => value,
                        Ok(None) => true,
                        Err(_) => return Err(()),
                    };
                let deadband =
                    match read_authored_telemetry_real(&reader, &path, "lunco:telemetry:deadband")?
                    {
                        None | Some(0.0) => None,
                        Some(value) if value > 0.0 => Some(value),
                        Some(_) => return Err(()),
                    };
                let retention = match reader
                    .attr_value(&path, "lunco:telemetry:retention")
                    .and_then(|value| value.get::<i64>())
                {
                    Some(0) | None
                        if !reader.has_authored_attribute(&path, "lunco:telemetry:retention") =>
                    {
                        None
                    }
                    Some(0) => None,
                    Some(value) if value > 0 => Some(usize::try_from(value).map_err(|_| ())?),
                    _ => return Err(()),
                };
                Ok((
                    Parameter {
                        name,
                        unit,
                        description,
                        source,
                        target: Some(target_entity),
                        rate_hz,
                        enabled,
                        deadband,
                        retention,
                    },
                    display_name,
                ))
            })();
            if let Ok((parameter, display_name)) = declaration {
                let (target, source) = match &parameter.source {
                    ChannelSource::Port(port) => {
                        let key = (
                            prim_path.stage_handle.id(),
                            instance_of(provenance, gid, is_root),
                            target_path.clone(),
                            port.clone(),
                        );
                        if let Some((wrapper, runtime_port)) = index.generated_outputs.get(&key) {
                            (Some(*wrapper), ChannelSource::Port(runtime_port.clone()))
                        } else {
                            // A domain member has no standalone port surface.
                            // Leave its declaration unprojected until the
                            // generated wrapper publishes the topology map;
                            // marking it now would permanently cache the wrong
                            // member target and produce a false missing-port
                            // warning during the compile window.
                            // A direct binding is valid only when the target
                            // has published its own port surface. A physical
                            // prim that is still waiting for a generated
                            // Modelica wrapper must remain pending; binding
                            // the authored member name here would create a
                            // channel that can never be read.
                            let direct_surface = target_surface_query
                                .get(target_entity)
                                .is_ok_and(|(sim, ready, pending)| !pending && (sim || ready));
                            // A USD Modelica member has no standalone port
                            // surface. Leave its declaration unprojected until
                            // the generated wrapper publishes the topology map;
                            // otherwise it would bind to the transform/member
                            // entity and emit a false missing-port warning.
                            if pending_interface_query.contains(target_entity) || !direct_surface {
                                continue;
                            }
                            (parameter.target, parameter.source.clone())
                        }
                    }
                    _ => (parameter.target, parameter.source.clone()),
                };
                let parameter = Parameter {
                    target,
                    source,
                    ..parameter
                };
                let mut channel = commands.spawn((
                    Name::new(format!("telemetry:{}", parameter.name)),
                    UsdTelemetryChannel,
                    ChildOf(entity),
                    parameter,
                ));
                if let Some(display_name) = display_name {
                    channel.try_insert(lunco_core::markers::Callsign(display_name));
                }
            } else {
                index.diagnostics.insert(
                    (id, path.as_str().to_owned()),
                    RuntimeDiagnostic {
                        code: "telemetry-contract".to_string(),
                        severity: DiagnosticSeverity::Error,
                        producer: "usd-telemetry".to_string(),
                        subject: path.as_str().to_owned(),
                        message: "telemetry declaration has invalid metadata; provide one non-empty lunco:telemetry:port or lunco:telemetry:reflect and valid numeric sampling settings".to_string(),
                    },
                );
                warn!(
                    "[usd-cosim] {} has invalid telemetry attributes; declaration ignored",
                    path.as_str()
                );
            }
        }
        commands.entity(entity).try_insert(UsdTelemetryProjected);
    }

    if let Some(mut diagnostics) = diagnostics {
        diagnostics.replace_producer("usd-telemetry", index.diagnostics.values().cloned());
    }
}

/// Reads one cosim prim's attributes and dispatches its model + wires + events
/// from the live composed [`lunco_usd_bevy_stage::UsdRead`] surface.
fn process_usd_cosim_prim_read(
    reader: &dyn UsdReadObject,
    entity: Entity,
    prim_path: &UsdPrimPath,
    sdf_path: &SdfPath,
    // Every prim some `CollectionAPI:components` scope on this stage owns.
    network_members: &BTreeSet<String>,
    commands: &mut Commands,
    asset_server: &AssetServer,
    wiring_dirty: &mut UsdWiringDirty,
    python_unavailable: &mut PythonUnavailablePrograms,
) {
    if reader.type_name(sdf_path).as_deref() == Some("LunCoEvent") {
        let sources = reader.connections(sdf_path, "inputs:trigger");
        let Some(source) = sources.first() else {
            warn!(
                "[usd-cosim] {}: LunCoEvent has no inputs:trigger connection",
                sdf_path
            );
            return;
        };
        let source = source.to_string();
        let Some((source_path, output)) = source.split_once(".outputs:") else {
            warn!(
                "[usd-cosim] {}: event trigger source `{source}` is not an outputs:* property",
                sdf_path
            );
            return;
        };
        let name = match reader.attr_value(sdf_path, "lunco:event:name") {
            Some(Value::Token(value)) if !value.as_str().is_empty() => value.to_string(),
            _ => {
                warn!(
                    "[usd-cosim] {}: LunCoEvent has no valid lunco:event:name",
                    sdf_path
                );
                return;
            }
        };
        let severity = match reader.attr_value(sdf_path, "lunco:event:severity") {
            Some(Value::Token(value)) => match sync::parse_event_severity(value.as_str()) {
                Some(severity) => severity,
                None => {
                    warn!(
                        "[usd-cosim] {}: LunCoEvent has invalid lunco:event:severity",
                        sdf_path
                    );
                    return;
                }
            },
            _ => {
                warn!(
                    "[usd-cosim] {}: LunCoEvent has invalid lunco:event:severity",
                    sdf_path
                );
                return;
            }
        };
        let latched = match read_authored_bool_strict(reader, sdf_path, "lunco:event:latched") {
            Ok(Some(value)) => value,
            Ok(None) => false,
            Err(_) => {
                warn!(
                    "[usd-cosim] {}: LunCoEvent has malformed lunco:event:latched",
                    sdf_path
                );
                return;
            }
        };
        let qualification_time_s = match reader.real(sdf_path, "lunco:event:qualificationTime") {
            Some(value) if value.is_finite() && value >= 0.0 => value,
            _ => {
                warn!(
                    "[usd-cosim] {}: LunCoEvent has invalid lunco:event:qualificationTime",
                    sdf_path
                );
                return;
            }
        };
        commands.entity(entity).try_insert(sync::EventBinding {
            source_path: source_path.to_string(),
            output: output.to_string(),
            name,
            severity,
            latched,
            qualification_time_s,
            qualified_for_s: 0.0,
            armed: true,
        });
        return;
    }

    if !reader.has_api_schema(sdf_path, "LunCoProgramAPI") {
        return;
    }

    // A member of a component collection is compiled INTO its network's
    // generated model by `lunco-usd-sim-domain`. Compiling it here as well would
    // create a second, physically independent solver for one authored
    // component, whose outputs then feed the wire fabric.
    //
    // MEMBERSHIP is the test, not "declares an acausal connector". The two look
    // alike only because every shipped member happens to have a pin: a
    // causal-only member (a controller, a PDU — which `read_network` accepts and
    // documents) has no `connectors:` at all, so the old gate handed it exactly
    // that second solver.
    if network_members.contains(&prim_path.path) {
        commands.entity(entity).try_insert(UsdSimProcessed);
        return;
    }
    // …and the converse. A part with an acausal pin that NO network owns cannot
    // be solved at all: its `.mo` is a component class whose pins only mean
    // something inside a `connect()` set, so there is nothing to run standalone.
    // A bare `connectors:p` is only the component's interface declaration. It is
    // valid on a catalogue part such as a motor selected with `power = "infinite"`
    // and makes no topology claim until a `.connect` opinion is authored. Skip
    // both forms here; report only the connected form because that one is an
    // actionable topology error.
    let (has_acausal_connectors, has_connected_acausal_connectors) =
        acausal_connector_state(reader, sdf_path);
    if has_acausal_connectors {
        if has_connected_acausal_connectors {
            warn!(
                "[usd-cosim] {}: declares acausal `connectors:*` but belongs to no \
                 CollectionAPI:components network, so no Modelica model is generated for it and it \
                 does not simulate. Add it to a network root's `collection:components:includes`.",
                prim_path.path
            );
        }
        commands.entity(entity).try_insert(UsdSimProcessed);
        return;
    }

    // Active-cosim gate: a prim is stepped iff it BOTH binds a behavior model
    // AND declares connectable ports (`inputs:`/`outputs:` attributes). Ports
    // with no model are a pure physics sink driven through its backend (a joint
    // receiving `inputs:angle`, a rigid body receiving `inputs:force_y`). Wiring
    // itself is native `connectionPaths`, derived by `rewire_usd_connections`
    // (the journaled, distributed path), never parsed here. A Modelica/Python
    // source with no declared interface is an authored source-only program; it
    // receives an observable terminal status instead of disappearing from the
    // cosim graph.
    // The shared USD resolver selects the source arm and dispatches by file
    // format. This crate owns only Modelica and Python participants; Rhai and
    // Rhai owns authored behavior; this crate only projects Modelica and Python
    // participants.
    let resolved = match lunco_usd_bevy_core::program::resolve_program(reader, sdf_path) {
        Ok(resolved) => resolved,
        Err(issue) => {
            warn!(
                "[usd-cosim] program {} is unresolved at {}: {}",
                prim_path.path, issue.property, issue.message
            );
            return;
        }
    };
    let (backend, modelica_path, python_path) = match (resolved.backend, resolved.source) {
        (
            lunco_usd_bevy_core::program::ProgramBackend::Modelica,
            lunco_usd_bevy_core::program::ProgramSource::Asset(path),
        ) => (
            lunco_usd_bevy_core::program::ProgramBackend::Modelica,
            Some(path),
            None,
        ),
        (
            lunco_usd_bevy_core::program::ProgramBackend::Python,
            lunco_usd_bevy_core::program::ProgramSource::Asset(path),
        ) => (
            lunco_usd_bevy_core::program::ProgramBackend::Python,
            None,
            Some(path),
        ),
        // A program this crate does not solve (a Rhai script or a built-in
        // driver) is somebody else's to run.
        _ => return,
    };
    let (mut inputs, outputs) = declared_interface(reader, sdf_path);
    if inputs.is_empty() && outputs.is_empty() {
        let model_name = modelica_path.as_deref().map_or_else(
            || format!("Python:{}", python_path.as_deref().unwrap_or("<source>")),
            |path| format!("Modelica:{path}"),
        );
        let reason = format!(
            "program `{}` has no declared inputs or outputs; add an explicit USD scalar interface before it can run",
            prim_path.path
        );
        commands.entity(entity).try_insert((
            UsdSimProcessed,
            lunco_core_session::NotPredictable,
            SimComponent {
                model_name,
                inputs,
                outputs,
                status: SimStatus::Error(reason.clone()),
                ..default()
            },
        ));
        wiring_dirty.0 = true;
        warn!("[usd-cosim] {reason}");
        commands.trigger(lunco_telemetry_core::TelemetryEvent {
            name: MODEL_CONFIGURATION_INVALID.into(),
            source: 0,
            severity: lunco_telemetry_core::Severity::Error,
            data: lunco_telemetry_core::TelemetryValue::String(reason),
            timestamp: 0.0,
            sim_secs: 0.0,
            sim_tick: 0,
        });
        return;
    }

    // Modelica is a continuous-time participant, not a render callback. The
    // communication period is an authored co-simulation policy on the composed
    // program prim. An omitted property resolves to the schema's documented
    // 0.1 s default; an explicit invalid value is a terminal scene error, not
    // an invitation to run under a different schedule.
    let communication_period_result = match modelica_path.as_ref() {
        None => Ok(None),
        Some(_) => {
            let authored =
                reader.has_authored_attribute(sdf_path, "lunco:program:communicationPeriod");
            lunco_modelica_runtime::resolve_communication_period_secs(
                authored,
                reader.real(sdf_path, "lunco:program:communicationPeriod"),
            )
            .map(Some)
            .map_err(|reason| {
                format!(
                    "{}: lunco:program:communicationPeriod is invalid: {reason}",
                    prim_path.path,
                )
            })
        }
    };
    let communication_period_secs = match communication_period_result {
        Ok(value) => value,
        Err(reason) => {
            let model_name = modelica_path
                .as_deref()
                .map_or_else(|| "Modelica".to_string(), |path| format!("Modelica:{path}"));
            commands.entity(entity).try_insert((
                UsdSimProcessed,
                lunco_core_session::NotPredictable,
                SimComponent {
                    model_name,
                    inputs,
                    outputs,
                    status: SimStatus::Error(reason.clone()),
                    ..default()
                },
            ));
            wiring_dirty.0 = true;
            error!("[usd-cosim] {reason}");
            commands.trigger(lunco_telemetry_core::TelemetryEvent {
                name: MODEL_CONFIGURATION_INVALID.into(),
                source: 0,
                severity: lunco_telemetry_core::Severity::Error,
                data: lunco_telemetry_core::TelemetryValue::String(reason),
                timestamp: 0.0,
                sim_secs: 0.0,
                sim_tick: 0,
            });
            return;
        }
    };

    // A Python source is not a usable cosim participant until its interpreter
    // is available. Check at the authoritative USD bind boundary, before
    // publishing a pending load or claiming the program is bound. This keeps
    // the runtime contract honest on binaries built without the Python feature
    // and on machines whose shared Python library cannot be loaded.
    if let Some(asset_path) = python_path.as_deref() {
        let python_available = {
            #[cfg(feature = "python")]
            {
                get_python_status() == PythonStatus::Available
            }
            #[cfg(not(feature = "python"))]
            {
                false
            }
        };
        if !python_available {
            let reason =
                format!("Python runtime unavailable; cannot run `{asset_path}` in this binary");
            python_unavailable.paths.insert(prim_path.path.clone());
            commands.entity(entity).try_insert((
                UsdSimProcessed,
                lunco_core_session::NotPredictable,
                SimComponent {
                    model_name: format!("Python:{asset_path}"),
                    inputs,
                    outputs,
                    status: SimStatus::Error(reason.clone()),
                    ..default()
                },
            ));
            warn!(
                "[usd-cosim] program {} unavailable ({asset_path}): {reason}",
                prim_path.path
            );
            commands.trigger(lunco_telemetry_core::TelemetryEvent {
                name: MODEL_DISPATCH_FAILED.into(),
                source: 0,
                severity: lunco_telemetry_core::Severity::Error,
                data: lunco_telemetry_core::TelemetryValue::String(reason),
                timestamp: 0.0,
                sim_secs: 0.0,
                sim_tick: 0,
            });
            // The terminal component still participates in topology resolution:
            // declared wires must see its published interface and its Error status
            // must be observable by the binding/readiness projection.
            wiring_dirty.0 = true;
            return;
        }
    }

    // `UsdSourcedCosim` already inserted above; add the cosim-only markers.
    //
    // NB: this stamps `UsdSimProcessed`, which makes `process_usd_sim_prims` skip this
    // prim — fine, because link/celestial projection is now its OWN system
    // (`project_celestial_comms_prims`), gated by its OWN marker, so a cosim antenna
    // still gets its `LinkNode`. The two concerns no longer race on one flag.
    commands.entity(entity).try_insert(UsdSimProcessed);

    // NOTE: there is no possessable/vessel tag to stamp. A prim's command CAPABILITY
    // comes from its `Controls` scope → `ControlBinding` + `InputPorts`, stamped in
    // the general USD translator (`lunco-usd-bevy`), which runs for every prim — not
    // here, which only sees model-bound cosim prims. The avatar domain owns the
    // semantic possession boundary and excludes the `Embodiment` endpoint; authority
    // arbitration remains independent. A lander's actuation backend is its
    // `SimComponent` manual-override ports (written by `SetPorts`).

    // Opaque-body guard, applied HERE (cosim intent is known the instant we
    // read `lunco:modelicaModel`/`lunco:pythonModel`) rather than only later
    // in `tag_cosim_opaque`, which waits for the asynchronously-wrapped
    // `SimComponent`. That async gap was a prediction-takeover race: on a
    // client, `maintain_predicted_dynamic` (scene-edit) could stamp a balloon
    // `PredictedDynamic` during the multi-frame window before `NotPredictable`
    // landed — once b99991dd dropped the `SkipContentStamp` structural guard,
    // `NotPredictable` became the SOLE membership guard, so a late stamp meant
    // the body got predicted (local physics + cosim forces) and diverged.
    // Stamping at prim-read time closes the window. No vessel-kind exception:
    // a body reaching here has connectable ports + a model, so its motion is
    // cosim-driven by definition (a locally-driven rover chassis never gains
    // a `SimComponent` — under the sub-prim-per-model convention its Modelica
    // subsystems live on child prims, not the moving body). Harmless on
    // non-`RigidBody` cosim prims (e.g. a joint-driven solar tracker): the
    // marker is inert where prediction never runs.
    commands
        .entity(entity)
        .try_insert(lunco_core_session::NotPredictable);

    // Source files are loaded through Bevy's `AssetServer`: on native it reads
    // from the workspace `assets/` source, on wasm it issues an HTTP fetch
    // against the same path. Either way the actual Compile dispatch
    // happens later, in `dispatch_loaded_modelica_sources` /
    // `dispatch_loaded_python_sources`, once the asset is ready.
    // See `docs/architecture/40-asset-io.md`.
    // USD is the public contract: publish the declared scalar interface into a
    // `SimComponent` at BIND — before the async source load — so a wire into a
    // declared port never transiently reads as unknown, WHATEVER the solver
    // language. This is the ONE publication path shared by every cosim solver
    // (Modelica, Python); they differ only in the loader they attach and, for
    // Modelica, the `UsdModelicaPortContract` the compiler later checks its DAE
    // interface against. Python used to skip this and ship an EMPTY interface,
    // so every wire into it (e.g. `signal` on an amplifier) false-warned — the
    // shared path is what keeps the languages from drifting again.
    // `dispatch_loaded_{modelica,python}_sources` flips the status live once the
    // source has loaded/compiled; until then `can_step()` holds a `Compiling`
    // component.
    strip_rigid_body_inputs(reader, sdf_path, &mut inputs);
    let model_name = match (&modelica_path, &python_path) {
        (Some(path), _) => {
            commands
                .entity(entity)
                .try_insert(UsdModelicaPortContract::new(
                    inputs.keys().cloned(),
                    outputs.keys().cloned(),
                ));
            path.clone()
        }
        (_, Some(path)) => format!("Python:{path}"),
        // Unreachable after backend classification. Kept total so a new backend
        // cannot silently skip interface publication.
        (None, None) => return,
    };
    commands.entity(entity).try_insert(SimComponent {
        model_name,
        parameters: Default::default(),
        inputs,
        outputs,
        status: SimStatus::Compiling,
        is_stepping: false,
    });
    if let Some(communication_period_secs) = communication_period_secs {
        commands.entity(entity).try_insert(UsdModelicaSchedule {
            communication_period_secs,
        });
    }
    if let Some(asset_path) = modelica_path {
        commands.entity(entity).try_insert(PendingModelicaSource {
            handle: asset_server.load(asset_path.clone()),
            asset_path,
            session_id: 0,
            resume_after_compile: true,
        });
    }
    #[cfg(feature = "python")]
    if let Some(asset_path) = python_path {
        commands.entity(entity).try_insert(PendingPythonSource {
            handle: asset_server.load(asset_path.clone()),
            asset_path,
        });
    }

    // The realtime promise — `lunco:program:realtimeSafe = true`. DECLARED, never
    // inferred: no amount of reading a model's source establishes how long it takes
    // to step. Absent ⇒ not promised, and `rewire_usd_connections` refuses it a
    // force/torque port on a client-predicted body (see
    // `docs/architecture/28-modelica-realtime-physics.md`).
    match read_authored_bool_strict(reader, sdf_path, "lunco:program:realtimeSafe") {
        Ok(Some(true)) => {
            commands
                .entity(entity)
                .try_insert(lunco_cosim_core::RealtimeSafe);
        }
        Ok(Some(false)) | Ok(None) => {}
        Err(_) => warn!(
            "[usd-cosim] program {} has malformed `lunco:program:realtimeSafe`; promise ignored",
            prim_path.path
        ),
    }

    info!("[usd-cosim] program {} bound ({backend:?})", prim_path.path);
}

/// A `connectors:*` property declares an acausal Modelica interface, while its
/// connection list makes the topology claim that warrants an orphan warning.
/// Avoid allocating attribute names for the common connectorless program.
fn acausal_connector_state(reader: &dyn UsdReadObject, sdf_path: &SdfPath) -> (bool, bool) {
    if !reader.any_attr_with_prefix(sdf_path, "connectors:") {
        return (false, false);
    }

    let mut has_connector = false;
    for name in reader.attr_names(sdf_path) {
        if name.starts_with("connectors:") {
            has_connector = true;
            if !reader.connections(sdf_path, &name).is_empty() {
                return (true, true);
            }
        }
    }
    (has_connector, false)
}

/// Return an actionable discrepancy between USD's public causal boundary and
/// the interface actually accepted by the Modelica compiler.
fn modelica_port_contract_error(
    contract: &UsdModelicaPortContract,
    model: &ModelicaModel,
) -> Option<String> {
    let missing_inputs: Vec<_> = contract
        .inputs
        .difference(&model.compiled_input_names)
        // An unconnected USD `inputs:` value is also the authored parameter
        // boundary for a Modelica participant. Parameters are compile-time
        // values, not causal solver inputs, so the compiler correctly omits
        // them from `compiled_input_names`. Keep the contract check about
        // actual runtime wires; parameter admission is handled by the shared
        // USD-default projection below.
        .filter(|name| !model.parameters.contains_key(*name))
        // A single USD prim may be both a Modelica program and a physical
        // endpoint. In that shape `inputs:force_y` is the Avian body sink,
        // while `output Real force_y` is the Modelica actuator source. The
        // USD input is intentionally absent from the Modelica DAE; a same-
        // named compiled output proves this is the cross-domain loop rather
        // than a typo in a Modelica input name.
        .filter(|name| !model.variables.contains_key(*name))
        .cloned()
        .collect();
    let actual_outputs: BTreeSet<_> = model.variables.keys().cloned().collect();
    let missing_outputs: Vec<_> = contract
        .outputs
        .difference(&actual_outputs)
        .cloned()
        .collect();
    if missing_inputs.is_empty() && missing_outputs.is_empty() {
        return None;
    }

    let mut parts = Vec::new();
    if !missing_inputs.is_empty() {
        parts.push(format!(
            "USD inputs absent from compiled Modelica model: {}",
            missing_inputs.join(", ")
        ));
    }
    if !missing_outputs.is_empty() {
        parts.push(format!(
            "USD outputs absent from compiled Modelica model: {}",
            missing_outputs.join(", ")
        ));
    }
    Some(parts.join("; "))
}

/// Validate a USD program's declared causal ports once its DAE exists.
///
/// This runs after the Modelica worker response and before it can receive the
/// next step. A failed contract pauses the model and projects as one durable
/// `SimStatus::Error`; a fresh compiler session is checked again.
fn validate_usd_modelica_port_contracts(
    mut commands: Commands,
    mut q: Query<(
        Entity,
        &UsdModelicaPortContract,
        &mut ModelicaModel,
        Option<&ValidatedUsdModelicaPortContract>,
    )>,
    mut notices: MessageWriter<lunco_modelica_runtime::ModelicaNotice>,
) {
    for (entity, contract, mut model, validated) in &mut q {
        if model.is_compiling || !model.is_compiled {
            continue;
        }
        if validated.is_some_and(|state| state.session_id == model.session_id) {
            continue;
        }

        if let Some(error) = modelica_port_contract_error(contract, &model) {
            model.paused = true;
            model.last_error = Some(error.clone());
            notices.write(lunco_modelica_runtime::ModelicaNotice {
                level: lunco_modelica_runtime::NoticeLevel::Error,
                text: format!(
                    "[{}] USD/Modelica port contract error: {error}",
                    model.model_name
                ),
            });
        }
        commands
            .entity(entity)
            .try_insert(ValidatedUsdModelicaPortContract {
                session_id: model.session_id,
            });
    }
}

/// Drain `PendingModelicaSource` for entities whose `.mo` text has
/// finished loading via `AssetServer`. Parses the source, populates a
/// `ModelicaModel` stub, dispatches `ModelicaCommand::Compile`, and
/// removes the pending marker. Stable retry behaviour: if the asset
/// isn't ready this frame we just skip — the system runs again next
/// frame.
pub(crate) fn dispatch_loaded_modelica_sources(
    mut commands: Commands,
    mut q: Query<(
        Entity,
        &PendingModelicaSource,
        &UsdPrimPath,
        &mut SimComponent,
        Option<&UsdInputDefaults>,
        Option<&UsdModelicaSchedule>,
        Option<&mut ModelicaModel>,
    )>,
    sources: Res<Assets<ModelicaSource>>,
    asset_server: Res<AssetServer>,
    channels: Option<Res<ModelicaChannels>>,
    mut source_roots: Option<ResMut<lunco_modelica_source_roots::SourceRootRegistry>>,
    mut notices: MessageWriter<lunco_modelica_runtime::ModelicaNotice>,
    // The solver-selection input only carries the authored prediction contract.
    // Solver capability and Modelica lowering remain owned by the worker's
    // backend registry; they are never inferred from a DAE shape here.
    q_realtime_safe: Query<&lunco_cosim_core::RealtimeSafe>,
) {
    let Some(channels) = channels else { return };

    // ORDER MATTERS, so it must not be luck. The Modelica worker compiles
    // serially, so whichever model is sent first is the first to become usable
    // — and a scene where a plume-photometry model happens to be dispatched
    // ahead of a lander's guidance leaves the vehicle waiting behind a model
    // nothing depends on. Query iteration follows archetype order, which is not
    // stable run to run: MEASURED, two runs of `landing_legs.usda`
    // dispatched the same three models in different orders, and the vehicle was
    // ready at 0.80 s in one and not at all within the test in the other.
    //
    // Sorting by prim path makes the order a property of the SCENE rather than
    // of the ECS, which is what a deterministic runner needs.
    let mut pending: Vec<_> = q.iter_mut().collect();
    pending.sort_unstable_by(|(_, _, a, _, _, _, _), (_, _, b, _, _, _, _)| a.path.cmp(&b.path));

    for (entity, pending, prim_path, mut component, usd_defaults, schedule, current_model) in
        pending
    {
        // Bail loud if the asset failed to load — without this the
        // entity stays Pending forever and the user sees nothing.
        if asset_server.load_state(&pending.handle).is_failed() {
            let error = format!(
                "failed to load Modelica source `{}` via AssetServer",
                pending.asset_path
            );
            warn!("[usd-cosim] {error}");
            notices.write(lunco_modelica_runtime::ModelicaNotice {
                level: lunco_modelica_runtime::NoticeLevel::Error,
                text: format!("[{}] Asset load error: {error}", component.model_name),
            });
            component.status = SimStatus::Error(error.clone());
            if let Some(mut model) = current_model {
                model.is_compiling = false;
                model.is_compiled = false;
                model.is_stepping = false;
                model.paused = true;
                model.last_error = Some(error.clone());
                model.resume_after_compile = false;
            }
            commands
                .entity(entity)
                .try_remove::<PendingModelicaSource>();
            continue;
        }
        let Some(src) = sources.get(&pending.handle) else {
            continue;
        };

        // The source asset loader prepares this interface on Bevy's async
        // compute pool. `ModelicaModel::inputs` is a write buffer seeded from
        // the authored interface, which
        // `wrap_modelica_into_simcomponent` copies into `SimComponent::inputs` —
        // the port surface a wire writes to.
        let model_name = src
            .interface
            .model_name
            .clone()
            .unwrap_or_else(|| "Model".into());
        let mut parameters = src.interface.parameters.clone();
        let mut inputs = src.interface.inputs.clone();
        // USD is the instance-authoring boundary. Apply its unconnected
        // scalar values to the correct Modelica variability class before the
        // first compile: parameters stay compile-time parameters, while
        // `input Real` values remain live solver inputs. This classification
        // is source-driven and reusable for every Modelica asset; it does not
        // encode sensor- or vehicle-specific names.
        if let Some(defaults) = usd_defaults {
            for (name, value) in &defaults.0 {
                if let Some(parameter) = parameters.get_mut(name) {
                    *parameter = *value;
                } else if let Some(input) = inputs.get_mut(name) {
                    *input = *value;
                } else {
                    warn!(
                        "[usd-cosim] {}: `inputs:{}` is authored but the Modelica source ({}) declares no parameter or input — the value is ignored",
                        prim_path.path, name, model_name,
                    );
                }
            }
        }
        // DISPATCH FIRST, then stub. NOT `let _ = send(..)`: a closed worker
        // channel means the compile is never attempted, and a `ModelicaModel`
        // with no `last_error` and no `variables` projects `SimStatus::Compiling`
        // *every tick* through `sync_modelica_outputs`/`modelica_status` — a
        // state nothing can move it out of, so the model silently never steps.
        // The failure therefore has to live on the MODEL (`last_error`), not on
        // the component, or the next tick overwrites it. Closed-channel
        // detection is `send(..).is_err()`, the same test
        // `lunco_modelica_source_roots::ensure_loaded` uses.
        let Some(schedule) = schedule else {
            let error = format!(
                "Modelica source `{}` has no projected co-simulation schedule",
                pending.asset_path
            );
            component.status = SimStatus::Error(error.clone());
            if let Some(mut model) = current_model {
                model.is_compiling = false;
                model.is_compiled = false;
                model.is_stepping = false;
                model.paused = true;
                model.last_error = Some(error.clone());
                model.resume_after_compile = false;
            }
            commands
                .entity(entity)
                .try_remove::<PendingModelicaSource>();
            error!("[usd-cosim] {error}");
            notices.write(lunco_modelica_runtime::ModelicaNotice {
                level: lunco_modelica_runtime::NoticeLevel::Error,
                text: format!("[{}] {error}", component.model_name),
            });
            commands.trigger(lunco_telemetry_core::TelemetryEvent {
                name: MODEL_CONFIGURATION_INVALID.into(),
                source: 0,
                severity: lunco_telemetry_core::Severity::Error,
                data: lunco_telemetry_core::TelemetryValue::String(error),
                timestamp: 0.0,
                sim_secs: 0.0,
                sim_tick: 0,
            });
            continue;
        };

        let parameter_overrides = usd_defaults
            .map(|defaults| {
                defaults
                    .0
                    .iter()
                    .filter(|(name, _)| parameters.contains_key(*name))
                    .map(|(name, value)| (name.clone(), *value))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let root_admission = match source_roots.as_deref_mut() {
            Some(source_roots) => lunco_modelica_source_roots::admit_compile_roots(
                source_roots,
                src.interface.required_source_roots.iter().cloned(),
                &channels,
            ),
            None if src.interface.required_source_roots.is_empty() => Ok(()),
            None => Err("Modelica source-root registry is not installed".to_owned()),
        };
        let dispatch_error = root_admission
            .err()
            .map(|error| format!("could not admit Modelica source roots: {error}"))
            .or_else(|| {
                channels
                    .tx
                    .send(ModelicaCommand::Compile {
                        entity,
                        session_id: pending.session_id,
                        model_name: model_name.clone(),
                        source: src.text.clone(),
                        // Stable per-asset session URI (its asset path) — keeps this
                        // model's overlay distinct in the worker session and consistent
                        // across recompiles. See `ModelicaCommand::Compile::doc_uri`.
                        doc_uri: pending.asset_path.to_string(),
                        extra_sources: Vec::new(),
                        parameter_overrides,
                        stream: None,
                        // Declared, never inferred. A program without the promise is
                        // authoritative live co-simulation, not client prediction.
                        realtime_safe: q_realtime_safe.contains(entity),
                    })
                    .err()
                    .map(|_| {
                        format!(
                            "Modelica worker channel closed — `{}` was never compiled \
                             and will never step",
                            pending.asset_path
                        )
                    })
            });

        component.parameters = parameters.clone();
        component.inputs = inputs.clone();
        commands.entity(entity).try_insert(ModelicaModel {
            model_name: model_name.clone(),
            source_uri: pending.asset_path.clone(),
            parameters,
            inputs,
            communication_period_secs: schedule.communication_period_secs,
            // Durable verdict: `modelica_status` reads this first, so the
            // component reports `Error` on every subsequent tick instead of
            // reverting to `Compiling`.
            last_error: dispatch_error.clone(),
            // USD-cosim models are part of the live scene (balloon
            // buoyancy, the solar tracker) — they should simulate as soon
            // as they compile, not land paused. The doc/UI Run path doesn't
            // reach them (they have no DocumentId), so without this they
            // would stay frozen forever. The worker's compile-success
            // handler sets `paused = !resume_after_compile`.
            is_compiling: dispatch_error.is_none(),
            session_id: pending.session_id,
            paused: !pending.resume_after_compile,
            resume_after_compile: pending.resume_after_compile && dispatch_error.is_none(),
            ..default()
        });

        if let Some(error) = dispatch_error {
            error!("[usd-cosim] {error}");
            notices.write(lunco_modelica_runtime::ModelicaNotice {
                level: lunco_modelica_runtime::NoticeLevel::Error,
                text: format!("[{model_name}] {error}"),
            });
            // Immediate verdict for this tick; `modelica_status` keeps it from
            // the following one.
            component.status = SimStatus::Error(error.clone());
            commands.trigger(lunco_telemetry_core::TelemetryEvent {
                name: MODEL_DISPATCH_FAILED.into(),
                source: 0,
                severity: lunco_telemetry_core::Severity::Error,
                data: lunco_telemetry_core::TelemetryValue::String(error),
                timestamp: 0.0,
                sim_secs: 0.0,
                sim_tick: 0,
            });
        }

        commands
            .entity(entity)
            .try_remove::<PendingModelicaSource>();
    }
}

/// Drain `PendingPythonSource` analogously to the Modelica version.
#[cfg(feature = "python")]
fn python_source_load_error(asset_path: &str) -> String {
    format!("failed to load Python source `{asset_path}` via AssetServer")
}

#[cfg(feature = "python")]
fn mark_python_source_load_failed(sim: &mut SimComponent, error: &str) {
    // The bind-time interface is still valid, but the executable source is not.
    // This terminal state releases the binding epoch and readiness producer;
    // leaving `Compiling` here would wait forever for a source that cannot arrive.
    sim.status = SimStatus::Error(error.to_owned());
}

#[cfg(feature = "python")]
pub fn dispatch_loaded_python_sources(
    mut commands: Commands,
    q: Query<(Entity, &PendingPythonSource)>,
    sources: Res<Assets<PythonSource>>,
    asset_server: Res<AssetServer>,
    mut registry: ResMut<ScriptRegistry>,
    mut notices: MessageWriter<lunco_modelica_runtime::ModelicaNotice>,
    // The `SimComponent` was published at BIND with the USD-declared interface;
    // dispatch reads it to seed the editor document and flips it live.
    mut sims: Query<&mut SimComponent>,
) {
    for (entity, pending) in q.iter() {
        if asset_server.load_state(&pending.handle).is_failed() {
            let error = python_source_load_error(&pending.asset_path);
            let model_name = if let Ok(mut sim) = sims.get_mut(entity) {
                let model_name = sim.model_name.clone();
                mark_python_source_load_failed(&mut sim, &error);
                model_name
            } else {
                format!("Python:{}", pending.asset_path)
            };
            warn!("[usd-cosim] {error}");
            notices.write(lunco_modelica_runtime::ModelicaNotice {
                level: lunco_modelica_runtime::NoticeLevel::Error,
                text: format!("[{model_name}] Asset load error: {error}"),
            });
            commands.trigger(lunco_telemetry_core::TelemetryEvent {
                name: MODEL_DISPATCH_FAILED.into(),
                source: 0,
                severity: lunco_telemetry_core::Severity::Error,
                data: lunco_telemetry_core::TelemetryValue::String(error),
                timestamp: 0.0,
                sim_secs: 0.0,
                sim_tick: 0,
            });
            commands.entity(entity).try_remove::<PendingPythonSource>();
            continue;
        }
        let Some(src) = sources.get(&pending.handle) else {
            continue;
        };

        // The editor document's declared I/O is the SAME contract already
        // published into `SimComponent` at bind — derive it from there rather than
        // hardcoding one model's ports (was `height`/`velocity`/`netForce`, wrong
        // for every other Python model).
        let (doc_inputs, doc_outputs) = sims
            .get(entity)
            .map(|sim| {
                let mut inputs: Vec<String> = sim.inputs.keys().cloned().collect();
                let mut outputs: Vec<String> = sim.outputs.keys().cloned().collect();
                inputs.sort();
                outputs.sort();
                (inputs, outputs)
            })
            .unwrap_or_default();

        let doc_id = DocumentId::fresh();
        // Route through the registry funnel so a journal recorder attaches (edits
        // to this cosim script record like any other domain).
        registry.insert_document(
            doc_id,
            ScriptDocument {
                id: doc_id.raw(),
                generation: 0,
                language: ScriptLanguage::Python,
                source: src.text.clone(),
                origin: DocumentOrigin::untitled(format!("Python-{}", doc_id.raw())),
                inputs: doc_inputs,
                outputs: doc_outputs,
                // No asset id: this source is SYNTHESIZED from a USD prim's inline
                // script, so it has no location for a relative `import` to anchor
                // against. `None` is the honest answer — an invented id would let a
                // relative import silently resolve against some unrelated root.
                asset_id: None,
                // Untitled, synthesized from a USD prim's inline source — never
                // on disk, so it is genuinely unsaved.
                last_saved_generation: None,
            },
        );
        commands.entity(entity).try_insert((
            ScriptedModel {
                document_id: Some(doc_id.raw()),
                language: Some(ScriptLanguage::Python),
                reload_policy: Default::default(),
                paused: false,
                parameters: Default::default(),
                parameters_revision: 0,
                inputs: Default::default(),
                outputs: Default::default(),
            },
            // This Python document was synthesized from a USD prim and has
            // the same scene ownership boundary as an embedded Rhai script.
            SceneOwnedScript,
        ));

        // Script loaded: flip the bind-published `SimComponent` live. It was
        // created `Compiling` at bind carrying the USD-declared interface — do NOT
        // re-create it here (that discarded the interface and made every wire into
        // this model false-warn as an unknown input). Python has no separate
        // compile step, so loaded ⇒ `Running`.
        if let Ok(mut sim) = sims.get_mut(entity) {
            sim.status = SimStatus::Running;
        }

        commands.entity(entity).try_remove::<PendingPythonSource>();
    }
}

/// On-model-BIND: publish the `SimComponent` — the entity's port interface —
/// as soon as a `ModelicaModel` exists, i.e. from the parse, not from the
/// compile.
///
/// A model's INTERFACE (`input Real …`, parameters) is a declaration; only its
/// SOLUTION (`variables`, the outputs) needs the solver. `dispatch_loaded_
/// modelica_sources` already extracts inputs+parameters from the AST when it
/// dispatches the compile, so the interface is known several hundred
/// milliseconds before the worker answers.
///
/// This used to wait for `variables` to populate before creating the
/// `SimComponent` at all. In that window the prim existed with NO ports, so
/// every wire into it (`sun_azimuth`, `panel_yaw`, `vehicle_throttle` on the
/// solar rover) hit `write_port` → `false` and the propagation master reported
/// a *dangling wire* — a diagnostic that means "this wire is wrong", raised for
/// wiring that was entirely correct. Worse, that master dedups its report per
/// PORT NAME for the process lifetime, so a load-time false positive
/// permanently silenced the real report for that name.
///
/// Publishing at bind time removes the window instead of tolerating it: the
/// ports exist for the first propagation tick, values land in
/// `SimComponent.inputs`, and `sync_modelica_inputs` hands them to the solver
/// for its first step. `SimStatus::Compiling` marks the interface as declared
/// but not yet solving, and `can_step()` already refuses to step it.
pub(crate) fn wrap_modelica_into_simcomponent(
    mut commands: Commands,
    q_new: Query<
        (Entity, &ModelicaModel, Option<&UsdModelicaPortContract>),
        (With<UsdSourcedCosim>, Without<SimComponent>),
    >,
    mut pending: ResMut<PendingModelicaWrapWork>,
) {
    let mut entities = pending.0.take_queued();
    if pending.0.take_initial_discovery() {
        // Cover unwrapped participants that predate this cosim projector.
        entities.extend(q_new.iter().map(|(entity, ..)| entity));
    }
    let mut candidates: Vec<_> = entities
        .into_iter()
        .filter_map(|entity| q_new.get(entity).ok())
        .collect();
    candidates.sort_unstable_by_key(|(entity, ..)| *entity);

    for (entity, model, contract) in candidates {
        let mut entity_commands = commands.entity(entity);
        entity_commands.try_insert(SimComponent {
            model_name: model.model_name.clone(),
            parameters: model.parameters.clone(),
            inputs: model.inputs.clone(),
            // Outputs are the SOLUTION — empty until the worker answers.
            // `sync_modelica_outputs` fills them and flips the status.
            outputs: model.variables.clone(),
            status: sync::modelica_status(model),
            is_stepping: model.is_stepping,
        });
        // Generated domain roots publish their ModelicaModel one Update before
        // this wrapper can expose the shared SimComponent port surface. Keep
        // telemetry and connection diagnostics in typed assembly-pending state
        // for that real lifecycle interval; the wrapper owns readiness once it
        // has been inserted.
        entity_commands.remove::<lunco_port_core::PortSurfacePending>();
        if let Some(contract) = contract {
            entity_commands.try_insert(DeclaredOutputPorts {
                names: contract.outputs.iter().cloned().collect(),
            });
        }
    }
}

/// The authored constants on a prim's unconnected `inputs:` ports — a model's
/// parameters, as USD stated them.
///
/// Kept as its own component rather than written straight into `SimComponent`
/// because the two arrive in either order: the wiring pass reads USD the frame the
/// prim spawns, while the `SimComponent` only exists once the model has been
/// fetched, compiled, and wrapped, which is several frames later on native and an
/// HTTP round-trip later on the web.
#[derive(Component, Debug, Clone, Default)]
pub struct UsdInputDefaults(pub HashMap<String, f64>);

/// Let the Modelica backend react when an authored model state revision changes.
///
/// The authoring layer publishes only the generic revision. This backend then
/// compares its own parsed parameter state and, only when a compile-time value
/// changed, sends the participant through the same pending-source/compile
/// boundary as initial admission. Generated network roots are deliberately
/// excluded because their `GeneratedModelicaSource` projection owns that
/// lifecycle. A changed runtime `input Real` remains a live value.
pub(crate) fn request_modelica_parameter_recompile(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut q: Query<
        (
            Entity,
            &UsdInputDefaults,
            &mut ModelicaModel,
            &mut SimComponent,
        ),
        (
            Changed<lunco_core::ModelStateRevision>,
            With<UsdSourcedCosim>,
            Without<GeneratedModelicaSource>,
            Without<PendingModelicaSource>,
        ),
    >,
) {
    for (entity, defaults, mut model, mut component) in &mut q {
        let parameter_changed = defaults.0.iter().any(|(name, value)| {
            model
                .parameters
                .get(name)
                .is_some_and(|current| current.to_bits() != value.to_bits())
        });
        if !parameter_changed || model.source_uri.is_empty() {
            continue;
        }

        let session_id = model.session_id.checked_add(1).unwrap_or(1);
        let resume_after_compile = !model.paused;
        let asset_path = model.source_uri.clone();

        model.session_id = session_id;
        model.is_compiling = true;
        model.is_compiled = false;
        model.is_stepping = false;
        model.in_flight_step = None;
        model.next_step_id = 1;
        model.compiled_input_names.clear();
        model.last_error = None;
        model.paused = true;
        model.resume_after_compile = resume_after_compile;
        model.current_time = 0.0;
        model.target_time = 0.0;
        model.next_communication_time = 0.0;
        model.last_step_time = 0.0;
        model.variables.clear();
        component.outputs.clear();
        component.status = SimStatus::Compiling;

        commands.entity(entity).try_insert(PendingModelicaSource {
            handle: asset_server.load(asset_path.clone()),
            asset_path,
            session_id,
            resume_after_compile,
        });
    }
}

/// Seed a model's inputs from the constants USD authored on its unconnected ports.
///
/// This is the ONLY path from USD to a model's parameters. Runs when the model
/// appears (`Added<SimComponent>`) and when the authored values change (a live edit
/// re-runs the wiring pass, which re-publishes [`UsdInputDefaults`]) — never on a
/// plain re-derive, so a value written by a script or the network is not undone.
///
/// A key the model does not declare is dropped by the port backend, so a typo'd
/// parameter is not a silent no-op: it is named here.
pub(crate) fn seed_usd_input_defaults(
    mut q: Query<
        (
            &UsdInputDefaults,
            &mut SimComponent,
            &UsdPrimPath,
            Option<&mut ModelicaModel>,
            Option<&UsdModelicaPortContract>,
        ),
        Or<(
            Added<SimComponent>,
            Added<ModelicaModel>,
            Changed<UsdInputDefaults>,
        )>,
    >,
) {
    for (defaults, mut sim, prim_path, model, modelica_contract) in q.iter_mut() {
        let mut model = model;
        for (port, value) in &defaults.0 {
            if let Some(model) = model.as_deref_mut() {
                if model.parameters.contains_key(port) {
                    model.parameters.insert(port.clone(), *value);
                    sim.parameters.insert(port.clone(), *value);
                } else if model.inputs.contains_key(port) {
                    model.inputs.insert(port.clone(), *value);
                    sim.inputs.insert(port.clone(), *value);
                } else {
                    warn!(
                        "[usd-cosim] {}: `inputs:{}` is authored but the Modelica model ({}) declares no parameter or input — the value is ignored",
                        prim_path.path, port, sim.model_name,
                    );
                }
            } else if modelica_contract.is_some() {
                // The Modelica source has not arrived yet. The USD-declared
                // placeholder interface intentionally does not decide whether
                // a name is a parameter or a runtime input; dispatch applies
                // the value once the parsed source contract is authoritative.
                continue;
            } else if sim.inputs.contains_key(port) {
                // Non-Modelica solvers retain the generic USD input surface.
                sim.inputs.insert(port.clone(), *value);
            } else {
                warn!(
                    "[usd-cosim] {}: `inputs:{}` is authored but the program ({}) declares no such input — the value is ignored",
                    prim_path.path, port, sim.model_name,
                );
            }
        }
    }
}

// API query providers live in `lunco-usd-sim-cosim-api`, keeping JSON serialization
// and API-facing dependency edges out of this runtime projection crate.

/// Registers translator systems, per-tick sync systems, and the API query
/// provider. This is a separate application plugin so the vehicle projector
/// does not depend on this heavy co-simulation implementation package.
///
/// Opaque-body guard (prediction-membership design in git history): stamp
/// [`lunco_core_session::NotPredictable`] on every cosim-driven physics body — one with a
/// [`SimComponent`] (its motion comes from Modelica/script forces the client does
/// not run) AND a [`RigidBody`]. This is the cosim **takeover** site: the same
/// `SimComponent`-attachment that makes a body server-driven also marks it
/// unpredictable, so the client's prediction systems (`maintain_predicted_dynamic`,
/// and any future contact-island promotion) refuse to ever predict it and keep it
/// on the interpolated proxy path. No vessel-kind exception: a `SimComponent` on
/// a `RigidBody` means the body's motion IS the cosim solver's output, which the
/// client can't reproduce. A locally-driven rover chassis never carries a
/// `SimComponent` (its Modelica subsystems live on child prims under the
/// sub-prim-per-model convention), so it is naturally excluded by topology.
/// Runs on both peers (cheap, idempotent — `Without<NotPredictable>` makes it a
/// one-shot per body); harmless where prediction never runs.
fn tag_cosim_opaque(
    mut commands: Commands,
    q: Query<
        Entity,
        (
            With<SimComponent>,
            With<avian3d::prelude::RigidBody>,
            Without<lunco_core_session::NotPredictable>,
        ),
    >,
) {
    for e in q.iter() {
        commands
            .entity(e)
            .try_insert(lunco_core_session::NotPredictable);
    }
}

/// Per-tick ordering inside `FixedUpdate` matches the cosim master
/// algorithm:
///   `ModelicaSet::HandleResponses (Update) → sync_*_outputs →
///    PropagateCosimSet::Propagate → ApplyForcesCosimSet::ApplyForces →
///    sync_*_inputs → ModelicaSet::SpawnRequests`.
impl Plugin for UsdSimCosimPlugin {
    fn build(&self, app: &mut App) {
        use lunco_cosim_core::schedule::{
            CosimApplySet as ApplyForcesCosimSet, CosimSet as PropagateCosimSet,
        };
        use lunco_modelica_runtime::ModelicaSet;

        // Script execution is part of the fixed co-simulation transaction. Its
        // input snapshot is taken after propagation/actuation, its output becomes
        // visible on the next tick, and the transaction completes before the
        // Modelica master dispatches the next communication point.
        app.configure_sets(
            FixedUpdate,
            lunco_scripting::ScriptingSet.before(ModelicaSet::SpawnRequests),
        );

        // Ensure the source asset types this module's systems read/allocate are
        // registered. Idempotent — production registers these via the Modelica /
        // scripting plugins; doing it here lets minimal apps (headless tests using
        // `MinimalPlugins` without those plugins) run the cosim systems without
        // panicking on a missing `Assets<…>` resource.
        app.init_asset::<ModelicaSource>();
        #[cfg(feature = "python")]
        app.init_asset::<PythonSource>();
        // The USD simulation projection owns these derived registries and
        // writes them even in a headless host.  Production Modelica setup also
        // initializes them, but minimal USD/physics apps intentionally omit
        // that plugin; keeping the resources here makes the projection
        // plugin's system contract complete and idempotent.
        app.init_resource::<lunco_doc_bevy::DocumentRegistry<lunco_modelica_document::ModelicaDocument>>()
            .init_resource::<lunco_modelica_runtime::generated_source::GeneratedModelicaSources>()
            .init_resource::<lunco_cosim_core::BindingRevision>()
            .init_resource::<lunco_core_runtime::SimulationBarrierParticipants>()
            .init_resource::<lunco_scripting::ScriptRegistry>()
            .init_resource::<UsdWiringDirty>()
            .init_resource::<BindingEpochDirty>()
            .init_resource::<BindingModelStatuses>()
            .init_resource::<PythonUnavailablePrograms>()
            .init_resource::<lunco_usd_sim_domain::MemberClasses>()
            .init_resource::<lunco_usd_sim_domain::DomainClassUsers>()
            .init_resource::<lunco_usd_sim_domain::PendingDomainProjections>()
            .init_resource::<lunco_usd_sim_domain::PendingDomainProjectionCandidates>()
            .init_resource::<lunco_usd_sim_domain::PendingGeneratedSourceDocuments>()
            .init_resource::<PendingUsdCosimPrimWork>()
            .init_resource::<PendingModelicaWrapWork>()
            .init_resource::<WiringFactsCache>()
            .init_resource::<lunco_usd_sim_domain::synthesis::SynthesizerRegistry>()
            .init_resource::<UsdTelemetryProjectionIndex>();
        app.world_mut().resource_mut::<UsdWiringDirty>().0 = true;
        app.world_mut()
            .resource_mut::<lunco_modelica_runtime::generated_source::GeneratedModelicaSources>()
            .dirty = true;
        app.add_observer(request_binding_epoch::<UsdPrimPath>)
            .add_observer(request_binding_epoch_on_remove::<UsdPrimPath>)
            .add_observer(invalidate_usd_telemetry_projection_index_on_insert::<UsdPrimPath>)
            .add_observer(invalidate_usd_telemetry_projection_index_on_remove::<UsdPrimPath>)
            .add_observer(
                invalidate_usd_telemetry_projection_index_on_insert::<GeneratedModelicaSource>,
            )
            .add_observer(
                invalidate_usd_telemetry_projection_index_on_remove::<GeneratedModelicaSource>,
            )
            .add_observer(
                invalidate_usd_telemetry_projection_index_on_insert::<ModelicaSignalLayout>,
            )
            .add_observer(
                invalidate_usd_telemetry_projection_index_on_remove::<ModelicaSignalLayout>,
            )
            .add_observer(invalidate_usd_telemetry_projection_index_on_insert::<SimComponent>)
            .add_observer(invalidate_usd_telemetry_projection_index_on_remove::<SimComponent>)
            .add_observer(
                invalidate_usd_telemetry_projection_index_on_insert::<lunco_port_core::PortSurface>,
            )
            .add_observer(
                invalidate_usd_telemetry_projection_index_on_remove::<lunco_port_core::PortSurface>,
            )
            .add_observer(
                invalidate_usd_telemetry_projection_index_on_insert::<
                    lunco_port_core::PortSurfaceReady,
                >,
            )
            .add_observer(
                invalidate_usd_telemetry_projection_index_on_remove::<
                    lunco_port_core::PortSurfaceReady,
                >,
            )
            .add_observer(queue_added_usd_cosim_prim)
            .add_observer(forget_removed_usd_cosim_prim)
            .add_observer(queue_removed_usd_sourced_cosim)
            .add_observer(queue_modelica_wrap_for_new_model)
            .add_observer(queue_modelica_wrap_for_new_cosim_owner)
            .add_observer(queue_modelica_wrap_after_surface_removal)
            .add_observer(forget_removed_modelica_wrap_source)
            .add_observer(forget_removed_modelica_wrap_owner)
            .add_observer(lunco_usd_sim_domain::queue_added_domain_prim)
            .add_observer(lunco_usd_sim_domain::queue_added_domain_identity)
            .add_observer(lunco_usd_sim_domain::queue_removed_domain_identity)
            .add_observer(lunco_usd_sim_domain::queue_added_domain_instance_projection)
            .add_observer(lunco_usd_sim_domain::queue_removed_domain_instance_projection)
            .add_observer(lunco_usd_sim_domain::forget_domain_projection_entity)
            .add_observer(lunco_usd_sim_domain::queue_generated_source_document_sync)
            .add_observer(lunco_usd_sim_domain::queue_model_document_sync_for_generated_source)
            .add_observer(lunco_usd_sim_domain::forget_generated_source_document_sync)
            .add_observer(lunco_usd_sim_domain::mark_generated_sources_dirty_on_insert)
            // Link port names are derived from the classes of the other authored
            // LinkNodes. A node arriving after its wire must therefore reopen the
            // same binding transaction as any other projected endpoint.
            .add_observer(request_binding_epoch::<lunco_celestial_spatial_core::LinkNode>)
            .add_observer(request_binding_epoch_on_remove::<lunco_celestial_spatial_core::LinkNode>)
            .add_observer(request_binding_epoch::<ModelicaModel>)
            .add_observer(request_binding_epoch_on_remove::<ModelicaModel>)
            .add_observer(lunco_usd_sim_domain::on_remove_generated_source)
            .add_observer(request_binding_epoch::<SimComponent>)
            .add_observer(forget_binding_model_status)
            .add_observer(request_binding_epoch::<lunco_usd_avian_contracts::PendingUsdJoint>)
            .add_observer(
                request_binding_epoch_on_remove::<lunco_usd_avian_contracts::PendingUsdJoint>,
            )
            .add_observer(request_binding_epoch::<PendingDifferential>)
            .add_observer(request_binding_epoch_on_remove::<PendingDifferential>)
            .add_observer(request_binding_epoch::<SimConnection>)
            .add_observer(request_binding_epoch_on_remove::<SimConnection>);
        install_wiring_invalidation_observers(app);
        // USD source-load and contract failures use the same core notice stream as
        // the Modelica compiler, so the workbench console has one observable error
        // surface. `add_message` is idempotent when the Modelica plugin registered
        // it already.
        app.add_message::<lunco_modelica_runtime::ModelicaNotice>();
        // A scene that is still spawning, and an object whose model has not
        // compiled, are the two things this module knows are not ready. Declaring
        // them is part of driving them — see `crate::readiness`.
        app.add_plugins(crate::readiness::UsdReadinessPlugin);

        app.configure_sets(
            Update,
            (
                // Physics projection publishes the generic body/joint/wheel
                // surfaces (including synthesized wheel ports) in deferred ECS
                // commands.  Co-sim scene discovery must observe that completed
                // projection before it derives and binds USD connections; otherwise
                // the first binding epoch targets the source prim instead of its
                // authored OutputPorts/PortSurface contract.
                CosimUpdateSet::Scene.after(UsdSimSet::Projection),
                CosimUpdateSet::Projection,
                CosimUpdateSet::Wiring,
            )
                .chain()
                .after(lunco_usd_bevy_scene::UsdVisualProjectionSet),
        );

        app.add_systems(
            Update,
            settle_binding_epoch
                .after(CosimUpdateSet::Projection)
                // Dynamic bodies are held kinematic while USD joints and the
                // authored initial velocity are admitted.  The sealed epoch is
                // the initial-sample boundary, so it must be decided after that
                // admission system has published the final physics state; otherwise
                // an already-valid Avian wire can capture zero velocity/identity
                // attitude and never revisit the handoff.
                .after(UsdSimSet::ActivateDynamicBodies)
                .before(CosimUpdateSet::Wiring)
                .run_if(|dirty: Res<BindingEpochDirty>| dirty.0),
        );
        app.add_systems(
            Update,
            request_binding_epoch_on_model_change
                .after(CosimUpdateSet::Wiring)
                .run_if(|changed: Query<(), Changed<SimComponent>>| !changed.is_empty()),
        );

        app.add_systems(
            Update,
            (
                // Gated on `any unprocessed cosim prim`: stay dormant
                // after scene-load is complete. Same archetype-check
                // pattern used for `process_usd_sim_prims`.
                process_usd_cosim_prims.run_if(any_unprocessed_usd_cosim),
                // Project authored `lunco:telemetry:*` declarations once the live
                // composed stage is available. This is independent of co-sim model
                // discovery so physical/avian and Modelica channels use one sampler.
                // Reads the class each member's `.mo` declares, so the projector
                // below instantiates what the file says rather than what its path
                // implies. Before it in the chain: a class landing this frame should
                // project this frame.
                lunco_usd_sim_domain::resolve_member_classes,
            )
                .chain()
                .in_set(CosimUpdateSet::Scene),
        );

        // Python source-load drain runs every Update only when the Python feature
        // is compiled in; the source asset may take multiple frames to load.
        #[cfg(feature = "python")]
        app.add_systems(
            Update,
            dispatch_loaded_python_sources
                .after(process_usd_cosim_prims)
                .before(lunco_usd_sim_domain::resolve_member_classes)
                .in_set(CosimUpdateSet::Scene),
        );

        app.add_systems(
            Update,
            report_python_unavailable.after(CosimUpdateSet::Scene),
        );
        app.add_systems(lunco_core::SceneTeardown, reset_python_unavailable);
        app.add_systems(lunco_core::SceneTeardown, reset_usd_cosim_prim_work);
        app.add_systems(lunco_core::SceneTeardown, reset_modelica_wrap_work);
        app.add_systems(lunco_core::SceneTeardown, reset_wiring_facts_cache);
        app.add_systems(
            lunco_core::SceneTeardown,
            lunco_usd_sim_domain::reset_scene_projection_work,
        );
        app.add_systems(
            lunco_core::SceneTeardown,
            reset_usd_telemetry_projection_index,
        );

        app.add_systems(
            Update,
            lunco_usd_sim_domain::project_domain_islands
                .run_if(lunco_usd_sim_domain::domain_projection_due)
                .in_set(CosimUpdateSet::Projection),
        );
        app.add_systems(
            Update,
            lunco_usd_sim_domain::poll_domain_projection_tasks
                .after(lunco_usd_sim_domain::project_domain_islands)
                .in_set(CosimUpdateSet::Projection),
        );
        app.add_systems(
            Update,
            lunco_usd_sim_domain::sync_generated_network_documents
                .after(lunco_usd_sim_domain::poll_domain_projection_tasks)
                .run_if(lunco_usd_sim_domain::generated_source_document_sync_due)
                .in_set(CosimUpdateSet::Projection),
        );
        app.add_systems(
            Update,
            lunco_usd_sim_domain::publish_generated_sources
                .after(lunco_usd_sim_domain::sync_generated_network_documents)
                .run_if(lunco_usd_sim_domain::generated_sources_need_publish)
                .in_set(CosimUpdateSet::Projection),
        );

        // Wiring is derived from native `connectionPaths`: rebuilds the
        // `SimConnection` set whenever prims spawn/despawn (structural) or a
        // `connectionPaths` edit is drained (`UsdWiringDirty`); dormant otherwise.
        // Register the stages separately because `run_if` turns a system into a
        // schedule config and cannot participate in this Bevy version's chained
        // system tuple. The explicit dependencies retain the same ownership order
        // without relying on tuple arity or a second compatibility path.
        // Keep the deferred flushes inside the wiring transaction. Bevy 0.19's
        // native `chain` configuration inserts the required synchronization after
        // each command-producing stage. Registering an explicit `ApplyDeferred`
        // system here is incorrect: the schedule also inserts automatic flush
        // points for other ordered command systems, and the type-based system set
        // then becomes ambiguous during schedule initialization (most visibly in
        // offscreen recording startup).
        //
        // Parameters: the authored constants the wiring pass gathered off the
        // unconnected `inputs:` ports, pushed into the model once it exists. After
        // the wrap, because it needs the `SimComponent` to write into.
        //
        // Modelica compilation consumes compile-time parameter overrides from the
        // composed USD `inputs:` surface. It therefore belongs after the wiring
        // projection, not alongside source discovery in `Scene`: on a fast local
        // asset load, dispatching earlier compiled with the Modelica declaration's
        // zero default before `UsdInputDefaults` existed on the entity. The model
        // then had a truthful-looking solver but the wrong initial state, and no
        // later runtime input could repair that compile-time initialization value.
        //
        // Python has no compile-time parameter phase and remains in `Scene`; its
        // loaded source only installs the already-published generic interface.
        app.add_systems(
            Update,
            (
                rewire_usd_connections.run_if(wiring_due),
                wrap_modelica_into_simcomponent.run_if(any_pending_modelica_wrap),
                request_modelica_parameter_recompile,
                seed_usd_input_defaults,
                dispatch_loaded_modelica_sources,
                // Run the lifecycle trigger after wrapper/source publication,
                // because those systems may add the runtime surface in this
                // same chain after the domain projection pass has completed.
                // Component observers and scalar USD revisions admit this work;
                // the steady state does not query the entity population.
                mark_usd_telemetry_projection_index_dirty
                    .run_if(telemetry_projection_index_invalidation_due),
                // The wrapper publishes the generic SimComponent surface and the
                // authored output contract in this same lifecycle transaction.
                // Project authored telemetry only after that publication, so the
                // fixed-step sampler never observes a generated endpoint between
                // its Modelica identity and its public port surface.
                project_usd_telemetry
                    .after(wrap_modelica_into_simcomponent)
                    .after(mark_usd_telemetry_projection_index_dirty)
                    .run_if(telemetry_projection_needed),
            )
                // Rewire commands must land before the wrapper query, and the
                // wrapper's component insertion must land before defaults are
                // seeded. Native deferred synchronization preserves both
                // ownership boundaries without a duplicate ApplyDeferred node.
                .chain()
                .in_set(CosimUpdateSet::Wiring),
        );
        // §6 opaque guard: once a body is cosim-driven, mark it unpredictable after
        // the fresh SimComponent and authored defaults are visible.
        app.add_systems(
            Update,
            tag_cosim_opaque
                .after(seed_usd_input_defaults)
                .in_set(CosimUpdateSet::Wiring),
        );
        // The Modelica worker must know which entities are on the shared causal
        // path before the next FixedUpdate. This is a graph projection, not a
        // per-frame solver heuristic; it stays dormant until topology or endpoint
        // lifecycle changes.
        app.add_systems(
            Update,
            derive_causal_barrier_participants
                .after(CosimUpdateSet::Wiring)
                .run_if(causal_participants_changed),
        );

        app.add_systems(
            FixedUpdate,
            (
                validate_usd_modelica_port_contracts.before(sync::sync_modelica_outputs),
                // Scenario hooks read the public SimComponent surface directly.
                // Make the publication edge explicit: without this dependency a
                // fast cached compile could open the scenario gate and let Rhai
                // observe the wrapper before the first Modelica snapshot had been
                // copied into it. Cold runs happened to order the two systems the
                // other way, which made the first telemetry sample nondeterministic.
                sync::sync_modelica_outputs
                    .before(lunco_scripting::ScriptingSet)
                    .before(PropagateCosimSet::Propagate),
                // Script backends consume the input snapshot and publish their
                // output snapshot as one fixed-step transaction. Script outputs
                // are published before the propagation phase, then the backend
                // executes after this tick's propagated inputs. That gives the
                // explicit one-tick causal delay required for a conservative
                // discrete co-simulation exchange and avoids an algebraic
                // same-tick script/physics cycle.
                sync::sync_script_inputs
                    .after(PropagateCosimSet::Propagate)
                    .after(ApplyForcesCosimSet::ApplyForces)
                    .before(lunco_scripting::ScriptingSet)
                    .before(ModelicaSet::SpawnRequests),
                sync::sync_script_outputs
                    .before(lunco_scripting::ScriptingSet)
                    .before(PropagateCosimSet::Propagate),
                sync::sync_modelica_inputs
                    .after(ApplyForcesCosimSet::ApplyForces)
                    .before(ModelicaSet::SpawnRequests),
                // Modelica `when` bridge: edge-detect on fresh outputs, after they sync.
                sync::fire_connected_events
                    .after(lunco_core_runtime::SimTickSet)
                    .after(sync::sync_modelica_outputs)
                    .after(sync::sync_script_outputs)
                    .after(lunco_scripting::ScriptingSet)
                    .run_if(lunco_time::simulation_is_running),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::{
        EventBinding, copy_modelica_input_values, event_rising_edge, fire_connected_events,
        modelica_status, parse_event_severity,
    };
    #[derive(Resource, Default)]
    struct WiringRuns(usize);

    fn count_wiring_runs(mut runs: ResMut<WiringRuns>, mut dirty: ResMut<UsdWiringDirty>) {
        runs.0 += 1;
        dirty.0 = false;
    }

    #[test]
    fn wiring_gate_is_dormant_until_a_real_trigger() {
        let mut app = App::new();
        app.init_resource::<UsdWiringDirty>()
            .init_resource::<WiringRuns>();
        install_wiring_invalidation_observers(&mut app);
        app.add_systems(Update, count_wiring_runs.run_if(wiring_due));

        app.update();
        assert_eq!(app.world().resource::<WiringRuns>().0, 0);

        let non_usd_model = app
            .world_mut()
            .spawn(lunco_cosim_core::SimComponent::default())
            .id();
        app.update();
        app.world_mut()
            .entity_mut(non_usd_model)
            .remove::<lunco_cosim_core::SimComponent>();
        app.update();
        assert_eq!(
            app.world().resource::<WiringRuns>().0,
            0,
            "a non-USD SimComponent lifecycle cannot dirty the USD wiring projection"
        );

        let visual_only = app.world_mut().spawn(UsdPrimPath::default()).id();
        app.update();
        assert_eq!(app.world().resource::<WiringRuns>().0, 0);

        app.world_mut()
            .entity_mut(visual_only)
            .insert(lunco_port_core::PortSurfaceReady);
        app.update();
        assert_eq!(app.world().resource::<WiringRuns>().0, 1);

        app.update();
        assert_eq!(app.world().resource::<WiringRuns>().0, 1);

        app.world_mut()
            .entity_mut(visual_only)
            .insert(lunco_core::GlobalEntityId::from_raw(12));
        app.update();
        assert_eq!(app.world().resource::<WiringRuns>().0, 2);

        app.world_mut()
            .entity_mut(visual_only)
            .remove::<lunco_core::GlobalEntityId>();
        app.update();
        assert_eq!(app.world().resource::<WiringRuns>().0, 3);

        app.world_mut()
            .entity_mut(visual_only)
            .remove::<lunco_port_core::PortSurfaceReady>();
        app.update();
        assert_eq!(app.world().resource::<WiringRuns>().0, 4);

        app.world_mut().resource_mut::<UsdWiringDirty>().0 = true;
        app.update();
        assert_eq!(app.world().resource::<WiringRuns>().0, 5);

        let endpoint = app
            .world_mut()
            .spawn((UsdPrimPath::default(), lunco_port_core::PortSurfaceReady))
            .id();
        app.update();
        assert_eq!(app.world().resource::<WiringRuns>().0, 6);

        app.world_mut().entity_mut(endpoint).remove::<UsdPrimPath>();
        app.update();
        assert_eq!(app.world().resource::<WiringRuns>().0, 7);

        app.update();
        assert_eq!(app.world().resource::<WiringRuns>().0, 7);
    }

    #[test]
    fn cosim_prim_discovery_tracks_only_unprocessed_lifecycles() {
        let mut app = App::new();
        app.init_resource::<PendingUsdCosimPrimWork>();
        app.world_mut()
            .resource_mut::<PendingUsdCosimPrimWork>()
            .0
            .take_initial_discovery();
        app.add_observer(queue_added_usd_cosim_prim)
            .add_observer(forget_removed_usd_cosim_prim)
            .add_observer(queue_removed_usd_sourced_cosim);

        let unprocessed = app.world_mut().spawn(UsdPrimPath::default()).id();
        assert!(
            app.world()
                .resource::<PendingUsdCosimPrimWork>()
                .0
                .contains(unprocessed)
        );

        let already_sourced = app
            .world_mut()
            .spawn((UsdPrimPath::default(), UsdSourcedCosim))
            .id();
        assert!(
            !app.world()
                .resource::<PendingUsdCosimPrimWork>()
                .0
                .contains(already_sourced)
        );

        app.world_mut()
            .entity_mut(unprocessed)
            .remove::<UsdPrimPath>();
        assert!(
            !app.world()
                .resource::<PendingUsdCosimPrimWork>()
                .0
                .contains(unprocessed)
        );

        app.world_mut()
            .entity_mut(already_sourced)
            .remove::<UsdSourcedCosim>();
        assert!(
            app.world()
                .resource::<PendingUsdCosimPrimWork>()
                .0
                .contains(already_sourced)
        );
    }

    #[test]
    fn modelica_wrap_work_tracks_only_unwrapped_participant_lifecycles() {
        let mut app = App::new();
        app.init_resource::<PendingModelicaWrapWork>();
        app.world_mut()
            .resource_mut::<PendingModelicaWrapWork>()
            .0
            .take_initial_discovery();
        app.add_observer(queue_modelica_wrap_for_new_model)
            .add_observer(queue_modelica_wrap_for_new_cosim_owner)
            .add_observer(queue_modelica_wrap_after_surface_removal)
            .add_observer(forget_removed_modelica_wrap_source)
            .add_observer(forget_removed_modelica_wrap_owner);

        let owner = app.world_mut().spawn(UsdSourcedCosim).id();
        app.world_mut()
            .entity_mut(owner)
            .insert(ModelicaModel::default());
        assert!(
            app.world()
                .resource::<PendingModelicaWrapWork>()
                .0
                .contains(owner)
        );

        let model_first = app.world_mut().spawn(ModelicaModel::default()).id();
        app.world_mut()
            .entity_mut(model_first)
            .insert(UsdSourcedCosim);
        assert!(
            app.world()
                .resource::<PendingModelicaWrapWork>()
                .0
                .contains(model_first)
        );

        let already_wrapped = app
            .world_mut()
            .spawn((
                UsdSourcedCosim,
                ModelicaModel::default(),
                SimComponent::default(),
            ))
            .id();
        assert!(
            !app.world()
                .resource::<PendingModelicaWrapWork>()
                .0
                .contains(already_wrapped)
        );

        app.world_mut()
            .entity_mut(already_wrapped)
            .remove::<SimComponent>();
        assert!(
            app.world()
                .resource::<PendingModelicaWrapWork>()
                .0
                .contains(already_wrapped)
        );

        app.world_mut()
            .entity_mut(already_wrapped)
            .remove::<ModelicaModel>();
        assert!(
            !app.world()
                .resource::<PendingModelicaWrapWork>()
                .0
                .contains(already_wrapped)
        );

        app.world_mut()
            .entity_mut(owner)
            .remove::<UsdSourcedCosim>();
        assert!(
            !app.world()
                .resource::<PendingModelicaWrapWork>()
                .0
                .contains(owner)
        );
    }

    #[test]
    fn modelica_wrapper_bootstraps_existing_and_queues_new_participants() {
        let mut app = App::new();
        app.init_resource::<PendingModelicaWrapWork>()
            .add_observer(queue_modelica_wrap_for_new_model)
            .add_observer(queue_modelica_wrap_for_new_cosim_owner)
            .add_observer(queue_modelica_wrap_after_surface_removal)
            .add_observer(forget_removed_modelica_wrap_source)
            .add_observer(forget_removed_modelica_wrap_owner)
            .add_systems(
                Update,
                wrap_modelica_into_simcomponent.run_if(any_pending_modelica_wrap),
            );

        let preexisting = app
            .world_mut()
            .spawn((UsdSourcedCosim, ModelicaModel::default()))
            .id();
        app.update();
        assert!(app.world().get::<SimComponent>(preexisting).is_some());

        let arriving = app.world_mut().spawn(UsdSourcedCosim).id();
        app.world_mut()
            .entity_mut(arriving)
            .insert(ModelicaModel::default());
        app.update();
        assert!(app.world().get::<SimComponent>(arriving).is_some());
    }

    #[test]
    fn scene_teardown_retires_cosim_prim_discovery_work() {
        let mut app = App::new();
        let mut pending = PendingUsdCosimPrimWork::default();
        pending.0.queue(Entity::from_bits(1));
        app.insert_resource(pending)
            .add_systems(lunco_core::SceneTeardown, reset_usd_cosim_prim_work);

        app.world_mut().run_schedule(lunco_core::SceneTeardown);

        let pending = app.world().resource::<PendingUsdCosimPrimWork>();
        assert!(!pending.0.has_work());
    }

    #[test]
    fn scene_teardown_retires_modelica_wrapper_work() {
        let mut app = App::new();
        let mut pending = PendingModelicaWrapWork::default();
        pending.0.queue(Entity::from_bits(1));
        app.insert_resource(pending)
            .add_systems(lunco_core::SceneTeardown, reset_modelica_wrap_work);

        app.world_mut().run_schedule(lunco_core::SceneTeardown);

        assert!(
            !app.world()
                .resource::<PendingModelicaWrapWork>()
                .0
                .has_work()
        );
    }

    #[derive(Resource, Default)]
    struct TelemetryProjectionRuns(usize);

    fn count_telemetry_projection_runs(mut runs: ResMut<TelemetryProjectionRuns>) {
        runs.0 += 1;
    }

    #[test]
    fn telemetry_projection_gate_closes_after_scene_projection() {
        let mut app = App::new();
        app.init_resource::<UsdTelemetryProjectionIndex>()
            .init_resource::<TelemetryProjectionRuns>()
            .add_observer(invalidate_usd_telemetry_projection_index_on_insert::<UsdPrimPath>)
            .add_observer(invalidate_usd_telemetry_projection_index_on_remove::<UsdPrimPath>)
            .add_systems(
                Update,
                (
                    mark_usd_telemetry_projection_index_dirty
                        .run_if(telemetry_projection_index_invalidation_due),
                    count_telemetry_projection_runs.run_if(telemetry_projection_needed),
                )
                    .chain(),
            );

        // Initial dirty state performs one bootstrap projection for entities
        // that existed before the plugin and its observers were installed.
        app.update();
        assert_eq!(app.world().resource::<TelemetryProjectionRuns>().0, 1);
        app.world_mut()
            .resource_mut::<UsdTelemetryProjectionIndex>()
            .dirty = false;

        let entity = app.world_mut().spawn(UsdPrimPath::default()).id();
        app.update();
        assert_eq!(app.world().resource::<TelemetryProjectionRuns>().0, 2);

        // The production projector clears the dirty bit after rebuilding its
        // indexes. This focused gate test models that ownership edge without
        // pulling in the composed USD stage.
        app.world_mut()
            .resource_mut::<UsdTelemetryProjectionIndex>()
            .dirty = false;

        app.world_mut()
            .entity_mut(entity)
            .insert(UsdTelemetryProjected);
        app.update();
        assert_eq!(app.world().resource::<TelemetryProjectionRuns>().0, 2);

        app.world_mut().entity_mut(entity).remove::<UsdPrimPath>();
        app.update();
        assert_eq!(app.world().resource::<TelemetryProjectionRuns>().0, 3);
    }

    #[test]
    fn telemetry_stage_revision_removes_derived_channels_and_markers() {
        let mut app = App::new();
        app.init_resource::<UsdTelemetryProjectionIndex>()
            .insert_resource(lunco_usd_bevy_scene::UsdStageRevision(1))
            .add_systems(Update, mark_usd_telemetry_projection_index_dirty);
        let declaration = app.world_mut().spawn(UsdTelemetryProjected).id();
        let channel = app.world_mut().spawn(UsdTelemetryChannel).id();

        app.update();

        assert!(
            app.world()
                .get::<UsdTelemetryProjected>(declaration)
                .is_none()
        );
        assert!(app.world().get_entity(channel).is_err());
        assert!(app.world().resource::<UsdTelemetryProjectionIndex>().dirty);
    }

    #[test]
    fn causal_barrier_is_the_reverse_closure_of_stateful_sinks() {
        let mut world = World::new();
        world.init_resource::<lunco_core_runtime::SimulationBarrierParticipants>();
        let mut revision = lunco_cosim_core::BindingRevision::default();
        revision.sealed = true;
        world.insert_resource(revision);

        let coupled = world.spawn(ModelicaModel::default()).id();
        let intermediate = world.spawn_empty().id();
        let telemetry_only = world.spawn(ModelicaModel::default()).id();
        let body = world
            .spawn((
                avian3d::prelude::RigidBody::Dynamic,
                lunco_port_core::CausalStateSink,
            ))
            .id();

        world.spawn((
            SimConnection {
                start_element: coupled,
                end_element: intermediate,
                end_connector: "input".into(),
                ..Default::default()
            },
            ConnectionBinding::Bound,
        ));
        world.spawn((
            SimConnection {
                start_element: intermediate,
                end_element: body,
                end_connector: "force_y".into(),
                ..Default::default()
            },
            ConnectionBinding::Bound,
        ));

        derive_causal_barrier_participants(&mut world);

        let participants = world.resource::<lunco_core_runtime::SimulationBarrierParticipants>();
        assert!(participants.topology_ready);
        assert!(participants.entities.contains(&coupled));
        assert!(!participants.entities.contains(&telemetry_only));
        assert!(participants.requires_barrier(coupled));
        assert!(!participants.requires_barrier(telemetry_only));
    }

    #[test]
    fn unresolved_topology_keeps_the_barrier_fail_closed() {
        let mut world = World::new();
        world.init_resource::<lunco_core_runtime::SimulationBarrierParticipants>();
        let mut revision = lunco_cosim_core::BindingRevision::default();
        revision.sealed = true;
        world.insert_resource(revision);

        let model = world.spawn(ModelicaModel::default()).id();
        let body = world
            .spawn((
                avian3d::prelude::RigidBody::Dynamic,
                lunco_port_core::CausalStateSink,
            ))
            .id();
        world.spawn((SimConnection {
            start_element: model,
            end_element: body,
            end_connector: "force_y".into(),
            ..Default::default()
        },));

        derive_causal_barrier_participants(&mut world);

        let participants = world.resource::<lunco_core_runtime::SimulationBarrierParticipants>();
        assert!(!participants.topology_ready);
        assert!(participants.requires_barrier(model));
    }

    #[test]
    fn failed_edge_does_not_make_model_a_shared_clock_participant() {
        let mut world = World::new();
        world.init_resource::<lunco_core_runtime::SimulationBarrierParticipants>();
        let mut revision = lunco_cosim_core::BindingRevision::default();
        revision.sealed = true;
        world.insert_resource(revision);

        let model = world.spawn(ModelicaModel::default()).id();
        let body = world
            .spawn((
                avian3d::prelude::RigidBody::Dynamic,
                lunco_port_core::CausalStateSink,
            ))
            .id();
        world.spawn((
            SimConnection {
                start_element: model,
                end_element: body,
                end_connector: "force_y".into(),
                ..Default::default()
            },
            ConnectionBinding::Failed,
        ));

        derive_causal_barrier_participants(&mut world);

        let participants = world.resource::<lunco_core_runtime::SimulationBarrierParticipants>();
        assert!(participants.topology_ready);
        assert!(!participants.entities.contains(&model));
        assert!(!participants.requires_barrier(model));
    }

    // ── interface published at parse, not at solve ───────────────────
    //
    // The contract that killed the "dangling wire" false positive: a bound
    // model exposes its declared inputs BEFORE the worker has produced any
    // variables, so a wire into it resolves on the first propagation tick.

    #[test]
    fn usd_connection_properties_are_declared_as_scalar_ports() {
        assert_eq!(
            declared_port_name("inputs:target_mount_x.connect", "inputs:"),
            Some("target_mount_x".to_owned())
        );
        assert_eq!(
            declared_port_name("outputs:spacecraft_mount_z", "outputs:"),
            Some("spacecraft_mount_z".to_owned())
        );
        assert_eq!(declared_port_name("physics:mass", "inputs:"), None);
    }

    #[test]
    fn environment_probe_declares_fixed_fields_and_leaves_direction_ports_dynamic() {
        let outputs = environment_probe_interface();
        assert_eq!(
            outputs
                .names
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            lunco_cosim_core::ENVIRONMENT_PROBE_BASE_OUTPUTS
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
        );
        assert!(outputs.names.iter().all(|name| {
            lunco_environment::DirectionSourceId::from_mount_connector(name).is_none()
        }));
    }

    /// A model that has been parsed and dispatched but not yet solved:
    /// declared inputs, no variables.
    fn dispatched_but_unsolved() -> ModelicaModel {
        let mut m = ModelicaModel {
            model_name: "GeneratedNetwork".into(),
            ..default()
        };
        m.inputs.insert("drive_left".into(), 0.0);
        m.inputs.insert("drive_right".into(), 0.0);
        m
    }

    #[test]
    fn declared_inputs_are_exposed_before_the_solver_answers() {
        let model = dispatched_but_unsolved();
        assert!(model.variables.is_empty(), "precondition: not solved yet");

        let mut app = App::new();
        let e = app.world_mut().spawn((UsdSourcedCosim, model)).id();
        app.init_resource::<PendingModelicaWrapWork>().add_systems(
            Update,
            wrap_modelica_into_simcomponent.run_if(any_pending_modelica_wrap),
        );
        app.update();

        let comp = app
            .world()
            .get::<SimComponent>(e)
            .expect("the interface must be published at bind, not at compile-complete");
        assert!(
            comp.inputs.contains_key("drive_left") && comp.inputs.contains_key("drive_right"),
            "a wire into a declared input must resolve while the model still compiles; \
             got inputs {:?}",
            comp.inputs.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            comp.status,
            SimStatus::Compiling,
            "declared but unsolved is `Compiling` — outputs are not trustworthy yet"
        );
    }

    #[test]
    fn generated_wrapper_declares_member_outputs_before_first_snapshot() {
        let model = dispatched_but_unsolved();
        let contract = UsdModelicaPortContract::new(
            ["drive_left".to_string(), "drive_right".to_string()],
            [
                "soc".to_string(),
                "__member_Rig_x2f_Battery_terminal_voltage_v".to_string(),
            ],
        );
        let mut app = App::new();
        let entity = app
            .world_mut()
            .spawn((UsdSourcedCosim, model, contract))
            .id();
        app.init_resource::<PendingModelicaWrapWork>().add_systems(
            Update,
            wrap_modelica_into_simcomponent.run_if(any_pending_modelica_wrap),
        );

        app.update();

        let declared = app
            .world()
            .get::<DeclaredOutputPorts>(entity)
            .expect("the generated wrapper must publish its complete output contract");
        assert!(declared.names.contains("soc"));
        assert!(
            declared
                .names
                .contains("__member_Rig_x2f_Battery_terminal_voltage_v")
        );
    }

    #[test]
    fn status_tracks_compile_run_pause_and_failure() {
        let mut model = dispatched_but_unsolved();
        assert_eq!(modelica_status(&model), SimStatus::Compiling);
        model.is_compiled = true;
        model.current_time = lunco_core_runtime::SECS_PER_TICK;
        assert_eq!(modelica_status(&model), SimStatus::Running);
        model.paused = true;
        assert_eq!(modelica_status(&model), SimStatus::Paused);
        model.last_error = Some("singular system".into());
        assert_eq!(
            modelica_status(&model),
            SimStatus::Error("singular system".into())
        );
    }

    #[test]
    #[cfg(feature = "python")]
    fn failed_python_source_is_terminal_for_binding_readiness() {
        let mut sim = SimComponent {
            model_name: "Python:models/controller.py".into(),
            status: SimStatus::Compiling,
            ..default()
        };

        let error = python_source_load_error("models/controller.py");
        let model_name = sim.model_name.clone();
        mark_python_source_load_failed(&mut sim, &error);

        assert_eq!(model_name, "Python:models/controller.py");
        assert!(error.contains("models/controller.py"));
        assert_eq!(sim.status, SimStatus::Error(error));
        assert!(
            modelica_models_terminal(std::iter::once((None, Some(&sim)))),
            "a source that failed to load must release the binding epoch as a terminal error"
        );
    }

    #[test]
    fn binding_epoch_does_not_treat_unwrapped_compile_as_terminal() {
        let model = ModelicaModel::default();
        assert!(!modelica_models_terminal(std::iter::once((
            Some(&model),
            None
        ))));

        let compiling = SimComponent {
            status: SimStatus::Compiling,
            ..default()
        };
        assert!(!modelica_models_terminal(std::iter::once((
            None,
            Some(&compiling),
        ))));
        assert!(!modelica_models_terminal(std::iter::once((
            Some(&model),
            Some(&compiling),
        ))));

        let compiled = ModelicaModel {
            is_compiled: true,
            ..default()
        };
        assert!(modelica_models_terminal(std::iter::once((
            Some(&compiled),
            Some(&compiling),
        ))));

        let ready = SimComponent {
            status: SimStatus::Idle,
            ..default()
        };
        let ready_model = ModelicaModel {
            is_compiled: true,
            ..default()
        };
        assert!(modelica_models_terminal(std::iter::once((
            Some(&ready_model),
            Some(&ready),
        ))));
        assert!(modelica_models_terminal(std::iter::once((None, None))));
    }

    #[test]
    fn compiled_interface_rejects_usd_port_not_accepted_by_modelica() {
        let contract = UsdModelicaPortContract {
            inputs: ["throttle".to_string(), "typo".to_string()]
                .into_iter()
                .collect(),
            outputs: ["thrust".to_string()].into_iter().collect(),
        };
        let mut model = dispatched_but_unsolved();
        model.compiled_input_names = ["throttle".to_string()].into_iter().collect();
        model.variables.insert("mass".into(), 1.0);

        let error = modelica_port_contract_error(&contract, &model)
            .expect("a USD port the DAE does not expose must fail projection");
        assert!(error.contains("typo"));
        assert!(error.contains("thrust"));
    }

    #[test]
    fn compiled_interface_accepts_matching_usd_ports() {
        let contract = UsdModelicaPortContract {
            inputs: ["throttle".to_string()].into_iter().collect(),
            outputs: ["thrust".to_string()].into_iter().collect(),
        };
        let mut model = dispatched_but_unsolved();
        model.compiled_input_names = ["throttle".to_string()].into_iter().collect();
        model.variables.insert("thrust".into(), 42.0);

        assert_eq!(modelica_port_contract_error(&contract, &model), None);
    }

    #[test]
    fn compiled_interface_accepts_usd_parameter_defaults() {
        let contract = UsdModelicaPortContract {
            inputs: ["filter_time_constant_s".to_string()].into_iter().collect(),
            outputs: BTreeSet::new(),
        };
        let mut model = dispatched_but_unsolved();
        model
            .parameters
            .insert("filter_time_constant_s".into(), 0.02);

        assert_eq!(modelica_port_contract_error(&contract, &model), None);
    }

    #[test]
    fn compiled_output_can_share_a_usd_input_name_for_a_physical_sink() {
        let contract = UsdModelicaPortContract {
            inputs: ["force_y".to_string()].into_iter().collect(),
            outputs: BTreeSet::new(),
        };
        let mut model = dispatched_but_unsolved();
        model.compiled_input_names.clear();
        model.variables.insert("force_y".into(), 42.0);

        assert_eq!(
            modelica_port_contract_error(&contract, &model),
            None,
            "a same-prim USD physics sink must not be reported as a Modelica input"
        );
    }

    #[test]
    fn physical_sink_inputs_do_not_hide_same_named_modelica_outputs() {
        let mut model = dispatched_but_unsolved();
        model.inputs.insert("guidance_throttle".into(), 0.0);
        model
            .compiled_input_names
            .insert("guidance_throttle".into());
        let mut component = SimComponent::default();
        component.inputs.insert("guidance_throttle".into(), 0.75);
        component.inputs.insert("force_y".into(), 0.0);

        assert!(copy_modelica_input_values(&mut model, &component, None));

        assert_eq!(model.inputs.get("guidance_throttle"), Some(&0.75));
        assert!(
            !model.inputs.contains_key("force_y"),
            "a physical sink must remain outside the Modelica input map"
        );
        assert!(
            !copy_modelica_input_values(&mut model, &component, None),
            "unchanged inputs must not be recopied"
        );
    }

    #[test]
    fn stable_modelica_outputs_do_not_dirty_the_shared_component() {
        let mut world = World::new();
        let mut model = ModelicaModel {
            is_compiled: true,
            current_time: 1.0,
            ..Default::default()
        };
        model.variables.insert("force_y".into(), 4.0);
        let mut component = SimComponent {
            status: SimStatus::Running,
            ..Default::default()
        };
        component.outputs.insert("force_y".into(), 4.0);
        let entity = world.spawn((model, component, UsdSourcedCosim)).id();

        world.clear_trackers();
        world
            .run_system_cached(sync::sync_modelica_outputs)
            .expect("output sync system runs");
        assert!(
            !world
                .entity(entity)
                .get_ref::<SimComponent>()
                .expect("shared component remains present")
                .is_changed(),
            "an identical output snapshot must not publish a false change"
        );

        world
            .entity_mut(entity)
            .get_mut::<ModelicaModel>()
            .expect("model remains present")
            .variables
            .insert("force_y".into(), 5.0);
        world.clear_trackers();
        world
            .run_system_cached(sync::sync_modelica_outputs)
            .expect("output sync system runs");
        assert!(
            world
                .entity(entity)
                .get_ref::<SimComponent>()
                .expect("shared component remains present")
                .is_changed(),
            "a changed output sample must still publish a change"
        );
    }

    #[test]
    fn authored_command_surface_reaches_shared_modelica_input() {
        let mut model = dispatched_but_unsolved();
        model.inputs.insert("throttle".into(), 0.0);
        model.compiled_input_names.insert("throttle".into());
        let component = SimComponent::default();
        let command_surface = lunco_port_core::InputPorts::with_defaults([
            ("throttle".to_string(), 0.75),
            ("heading".to_string(), -0.2),
        ]);

        assert!(copy_modelica_input_values(
            &mut model,
            &component,
            Some(&command_surface)
        ));

        assert_eq!(model.inputs.get("throttle"), Some(&0.75));
        assert!(!model.inputs.contains_key("heading"));
        assert!(
            !copy_modelica_input_values(&mut model, &component, Some(&command_surface)),
            "unchanged authored command values must not be recopied"
        );
    }

    #[test]
    fn connected_event_fires_once_per_rising_edge_and_rearms_by_default() {
        let mut armed = true;
        let mut qualified = 0.0;
        assert!(!event_rising_edge(
            &mut armed,
            &mut qualified,
            0.0,
            false,
            0.0,
            1.0
        ));
        assert!(event_rising_edge(
            &mut armed,
            &mut qualified,
            0.0,
            false,
            0.5,
            1.0
        ));
        assert!(!event_rising_edge(
            &mut armed,
            &mut qualified,
            0.0,
            false,
            1.0,
            1.0
        ));
        assert!(!event_rising_edge(
            &mut armed,
            &mut qualified,
            0.0,
            false,
            0.49,
            1.0
        ));
        assert!(event_rising_edge(
            &mut armed,
            &mut qualified,
            0.0,
            false,
            1.0,
            1.0
        ));
    }

    #[test]
    fn latched_event_ignores_contact_chatter_after_first_rising_edge() {
        let mut armed = true;
        let mut qualified = 0.0;
        assert!(!event_rising_edge(
            &mut armed,
            &mut qualified,
            0.0,
            true,
            0.0,
            1.0
        ));
        assert!(event_rising_edge(
            &mut armed,
            &mut qualified,
            0.0,
            true,
            0.5,
            1.0
        ));
        assert!(!event_rising_edge(
            &mut armed,
            &mut qualified,
            0.0,
            true,
            0.49,
            1.0
        ));
        assert!(!event_rising_edge(
            &mut armed,
            &mut qualified,
            0.0,
            true,
            1.0,
            1.0
        ));
    }

    #[test]
    fn event_qualification_requires_contiguous_active_time() {
        let mut armed = true;
        let mut qualified = 0.0;
        assert!(!event_rising_edge(
            &mut armed,
            &mut qualified,
            0.5,
            true,
            1.0,
            0.2
        ));
        assert!(!event_rising_edge(
            &mut armed,
            &mut qualified,
            0.5,
            true,
            0.0,
            0.2
        ));
        assert_eq!(qualified, 0.0);
        assert!(!event_rising_edge(
            &mut armed,
            &mut qualified,
            0.5,
            true,
            1.0,
            0.2
        ));
        assert!(!event_rising_edge(
            &mut armed,
            &mut qualified,
            0.5,
            true,
            1.0,
            0.2
        ));
        assert!(event_rising_edge(
            &mut armed,
            &mut qualified,
            0.5,
            true,
            1.0,
            0.1
        ));
    }

    #[test]
    fn event_severity_rejects_unknown_tokens() {
        assert_eq!(parse_event_severity("not-a-severity"), None);
        assert_eq!(
            parse_event_severity("critical"),
            Some(lunco_telemetry_core::Severity::Critical)
        );
    }
}
