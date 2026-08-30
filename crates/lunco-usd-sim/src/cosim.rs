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

use avian3d::prelude::PhysicsTime;
use bevy::ecs::query::QueryState;
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_core::telemetry::{ChannelSource, Parameter};
use lunco_core::{
    on_command, register_commands, Avatar, Command, DiagnosticSeverity, LocalAvatar, OriginAnchor,
    RuntimeDiagnostic, RuntimeDiagnostics, SceneTransition, SceneTransitionAdmission,
    SceneTransitionAdmitted, SceneTransitionCompleted, SceneTransitionCoordinator,
    SceneTransitionFailed, SceneTransitionIntent, SceneTransitionRequest, WorldGrid,
};
use lunco_cosim::{ConnectionBinding, DeclaredOutputPorts, SimComponent, SimConnection, SimStatus};
use lunco_doc::{DocumentId, DocumentOrigin};
use lunco_modelica::source_asset::ModelicaSource;
use lunco_modelica::{
    ast_extract::parse_model_interface, ModelicaChannels, ModelicaCommand, ModelicaModel,
    ModelicaSignalLayout,
};
use lunco_render::SceneCamera;
use lunco_scripting::python::{get_python_status, PythonStatus};
use lunco_scripting::source_asset::PythonSource;
use lunco_scripting::{
    doc::{ScriptDocument, ScriptLanguage, ScriptedModel},
    scenario::ScenarioDriver,
    world_bridge::RhaiScenarioRuntime,
    SceneOwnedScript, ScriptRegistry,
};
use lunco_usd_bevy::{
    camera_switch::CameraContractStatus, read_authored_bool_strict, CanonicalStages,
    UsdAwaitingStage, UsdInstanceMember, UsdInstanceRoot, UsdPrimPath, UsdRead, UsdSceneRoot,
    UsdStageAsset,
};
use openusd::sdf::{Path as SdfPath, Value};
use std::collections::{BTreeSet, HashMap};

use crate::domain_projection::GeneratedModelicaSource;
use crate::UsdSimProcessed;

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
/// The declaration projector is triggered by scene/projection changes and by
/// unprojected prims.  Its wrapper-port index therefore
/// belong to the projection lifecycle, not to the per-frame query.  Keeping
/// them here makes the steady state an empty gated system instead of a full
/// ECS scan and a set of cloned USD-path keys every Update.
#[derive(Resource, Default)]
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
    dirty: bool,
}

fn mark_usd_telemetry_projection_index_dirty(
    mut index: ResMut<UsdTelemetryProjectionIndex>,
    added_prims: Query<(), Added<UsdPrimPath>>,
    changed_wrappers: Query<
        (),
        Or<(
            Added<GeneratedModelicaSource>,
            Changed<GeneratedModelicaSource>,
            Added<ModelicaSignalLayout>,
            Changed<ModelicaSignalLayout>,
        )>,
    >,
    stage_revision: Option<Res<lunco_usd_bevy::UsdStageRevision>>,
    projected: Query<Entity, With<UsdTelemetryProjected>>,
    channels: Query<Entity, With<UsdTelemetryChannel>>,
    mut commands: Commands,
) {
    let revision_changed = stage_revision
        .as_ref()
        .is_some_and(|revision| revision.0 != index.observed_stage_revision);
    if !added_prims.is_empty() || !changed_wrappers.is_empty() || revision_changed {
        index.dirty = true;
        index.diagnostics.clear();
        if let Some(revision) = stage_revision {
            index.observed_stage_revision = revision.0;
        }
        for entity in &projected {
            commands.entity(entity).remove::<UsdTelemetryProjected>();
        }
        for entity in &channels {
            commands.entity(entity).despawn();
        }
    }
}

fn telemetry_projection_index_changed(
    added_prims: Query<(), Added<UsdPrimPath>>,
    changed_wrappers: Query<
        (),
        Or<(
            Added<GeneratedModelicaSource>,
            Changed<GeneratedModelicaSource>,
            Added<ModelicaSignalLayout>,
            Changed<ModelicaSignalLayout>,
        )>,
    >,
    index: Res<UsdTelemetryProjectionIndex>,
    stage_revision: Option<Res<lunco_usd_bevy::UsdStageRevision>>,
) -> bool {
    !added_prims.is_empty()
        || !changed_wrappers.is_empty()
        || stage_revision
            .as_ref()
            .is_some_and(|revision| revision.0 != index.observed_stage_revision)
}

fn telemetry_projection_needed(
    index: Res<UsdTelemetryProjectionIndex>,
    pending: Query<(), (With<UsdPrimPath>, Without<UsdTelemetryProjected>)>,
) -> bool {
    index.dirty || !pending.is_empty()
}

fn reset_usd_telemetry_projection_index(mut index: ResMut<UsdTelemetryProjectionIndex>) {
    index.generated_outputs.clear();
    index.entities_by_path.clear();
    index.generated_entities_by_path.clear();
    index.diagnostics.clear();
    index.observed_stage_revision = 0;
    index.dirty = true;
}

/// Telemetry event published when a USD-declared model could not be handed to
/// the solver at all — the worker channel was closed, so the compile that
/// `SimStatus::Compiling` is waiting for will never be attempted.
///
/// Published at [`lunco_core::Severity::Error`] so the workbench status bar's
/// error-telemetry observer surfaces it. A scene whose models silently
/// never step is indistinguishable from a scene that is merely still compiling;
/// the difference has to reach the UI, not just the log.
pub const MODEL_DISPATCH_FAILED: &str = "MODEL_DISPATCH_FAILED";

/// Telemetry event for a USD program whose authored execution policy is
/// invalid. It is distinct from a worker dispatch failure: the source was
/// never admitted because the scene configuration itself is not executable.
pub const MODEL_CONFIGURATION_INVALID: &str = "MODEL_CONFIGURATION_INVALID";

/// Marker indicating a USD-driven cosim entity has been wired up by
/// `process_usd_cosim_prims`. Prevents the system from re-processing
/// the same entity on the same tick.
#[derive(Component, Default)]
pub struct UsdSourcedCosim;

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

/// The scalar interface authored on a USD Modelica program.
///
/// USD declares the public causal boundary early so connection propagation can
/// start while the source asset loads. The Modelica compiler remains the
/// authority: after a successful compile, this contract is checked against the
/// DAE-reported inputs and observed outputs before the model may step.
#[derive(Component, Clone, Debug)]
pub(crate) struct UsdModelicaPortContract {
    inputs: BTreeSet<String>,
    outputs: BTreeSet<String>,
}

/// The authored co-simulation schedule for a USD Modelica participant.
///
/// This is kept separate from the public port contract because the latter is
/// about names/types while this is master-algorithm timing. It is projected once
/// from the composed USD prim and carried into `ModelicaModel` when the source
/// asset becomes executable.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct UsdModelicaSchedule {
    pub communication_period_secs: f64,
}

impl UsdModelicaPortContract {
    /// The contract a USD-declared boundary makes, whatever declared it — a
    /// program prim's `inputs:`/`outputs:` attributes, or a projected network's
    /// wrapper boundary.
    pub(crate) fn new(
        inputs: impl IntoIterator<Item = String>,
        outputs: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            inputs: inputs.into_iter().collect(),
            outputs: outputs.into_iter().collect(),
        }
    }
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
    reader: &lunco_usd_bevy::StageView<'_>,
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
    reader: &lunco_usd_bevy::StageView<'_>,
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

/// Publish the complete runtime interface of an environment probe.
///
/// `LunCoEnvironmentProbeAPI` declares these outputs on the schema class. The
/// live OpenUSD `property_names()` query intentionally reports authored
/// properties, not properties inherited from a codeless API schema, so using
/// [`declared_interface`] alone would create an empty source component for the
/// usual empty `probe.usda` asset. The environment domain owns this connector
/// contract; its systems fill the values when the corresponding fact exists,
/// while the declared-output component keeps the no-data probe structurally
/// connected without inventing a sample.
fn environment_probe_interface() -> DeclaredOutputPorts {
    DeclaredOutputPorts {
        names: lunco_cosim::ENVIRONMENT_PROBE_OUTPUTS
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

/// Scene transition transaction: set when a scene load is dispatched, cleared
/// once sync_usd_visuals has drained every UsdAwaitingStage prim for that
/// scene's stage asset.
///
/// Admission is serialized by [`SceneTransitionCoordinator`]. A second request
/// never reclaims entities still owned by this asset/projection phase; it starts
/// only after this transaction publishes a completed or failed edge.
///
/// The transaction is keyed by stage AssetId (not path string), so the clearing
/// system can match it against UsdPrimPath::stage_handle.id() on draining
/// UsdAwaitingStage entities.
#[derive(Resource)]
pub struct SceneLoadInFlight {
    /// Asset-relative path of the in-flight scene.
    pub path: String,
    /// Stage asset id of the in-flight load. Only the matching explicit asset
    /// outcome may close this transaction.
    pub stage_id: bevy::asset::AssetId<UsdStageAsset>,
}

/// Authoritative terminal asset outcome for a mounted scene stage.
///
/// The USD asset boundary publishes this from Bevy's load/failure messages.
/// An already-loaded stage publishes `Loaded` from the mount command itself.
/// Scene completion therefore advances from explicit outcomes, never from a
/// per-frame readiness poll.
#[derive(Message, Debug, Clone)]
pub enum SceneStageAssetOutcome {
    Loaded {
        stage_id: bevy::asset::AssetId<UsdStageAsset>,
    },
    Failed {
        stage_id: bevy::asset::AssetId<UsdStageAsset>,
        error: String,
    },
}

/// Terminal stage outcomes survive until the bounded USD visual projection has
/// drained the stage's queued prims. A loaded asset is therefore not allowed to
/// close the scene transaction while descendants are still being projected.
#[derive(Resource, Default)]
struct PendingSceneStageOutcome {
    stage_id: Option<bevy::asset::AssetId<UsdStageAsset>>,
    outcome: Option<SceneStageAssetOutcome>,
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
}

/// Same for Python.
#[derive(Component)]
pub struct PendingPythonSource {
    pub handle: Handle<PythonSource>,
    pub asset_path: String,
}

/// Reads cosim attributes from USD prims and dispatches model
/// compilation + wires. Runs in `Update` after `sync_usd_visuals` so
/// `Transform` / `Mesh3d` / `Material` are already present.
/// Run condition: any `UsdPrimPath` entity still lacks `UsdSourcedCosim`.
fn any_unprocessed_usd_cosim(q: Query<(), (With<UsdPrimPath>, Without<UsdSourcedCosim>)>) -> bool {
    !q.is_empty()
}

/// Run condition: any `UsdSourcedCosim` modelica model still needs wrapping
/// into a `SimComponent`.
fn any_unwrapped_modelica(
    q: Query<
        (),
        (
            With<UsdSourcedCosim>,
            With<ModelicaModel>,
            Without<SimComponent>,
        ),
    >,
) -> bool {
    !q.is_empty()
}

fn publish_loaded_scene_stage_outcomes(
    mut events: MessageReader<AssetEvent<UsdStageAsset>>,
    mut outcomes: MessageWriter<SceneStageAssetOutcome>,
) {
    for event in events.read() {
        if let AssetEvent::LoadedWithDependencies { id } = event {
            outcomes.write(SceneStageAssetOutcome::Loaded { stage_id: *id });
        }
    }
}

fn publish_failed_scene_stage_outcomes(
    mut events: MessageReader<bevy::asset::AssetLoadFailedEvent<UsdStageAsset>>,
    mut outcomes: MessageWriter<SceneStageAssetOutcome>,
) {
    for event in events.read() {
        outcomes.write(SceneStageAssetOutcome::Failed {
            stage_id: event.id,
            error: event.error.to_string(),
        });
    }
}

/// Close the scene transaction from one explicit stage outcome.
///
/// Loaded outcomes arrive after `sync_usd_visuals`; failure outcomes arrive
/// after the USD asset boundary has retired parked prims. A loaded outcome is
/// retained when the bounded visual projection queue is still draining and is
/// committed at the first later `Last` edge with no awaiting prims.
fn record_scene_load_terminal_outcome(
    mut outcomes: MessageReader<SceneStageAssetOutcome>,
    in_flight: Option<Res<SceneLoadInFlight>>,
    coordinator: Res<SceneTransitionCoordinator>,
    q_awaiting: Query<&UsdPrimPath, With<UsdAwaitingStage>>,
    q_lights: Query<&bevy::light::DirectionalLight>,
    camera_contract: Option<Res<CameraContractStatus>>,
    mut pending: ResMut<PendingSceneStageOutcome>,
    mut commands: Commands,
) {
    let Some(g) = in_flight else {
        outcomes.read().for_each(drop);
        pending.stage_id = None;
        pending.outcome = None;
        return;
    };
    if pending.stage_id != Some(g.stage_id) {
        pending.stage_id = None;
        pending.outcome = None;
    }
    if let Some(matching) = outcomes
        .read()
        .filter(|outcome| match outcome {
            SceneStageAssetOutcome::Loaded { stage_id }
            | SceneStageAssetOutcome::Failed { stage_id, .. } => *stage_id == g.stage_id,
        })
        .last()
        .cloned()
    {
        pending.stage_id = Some(g.stage_id);
        pending.outcome = Some(matching);
    }
    let Some(outcome) = pending.outcome.clone() else {
        return;
    };
    let Some(transition) = coordinator.active().cloned() else {
        // The stage outcome is stale: the scene was cleared before its asset
        // terminal message arrived. The clear path already removed the load
        // identity, but keep this guard local so a malformed event cannot
        // panic the process.
        warn!("[scene] ignoring stage outcome without an active scene transaction");
        commands.remove_resource::<SceneLoadInFlight>();
        pending.stage_id = None;
        pending.outcome = None;
        return;
    };
    if !matches!(
        &transition,
        SceneTransition::Load { .. } | SceneTransition::Restart { .. }
    ) {
        // A load identity attached to a non-load transaction is a lifecycle
        // violation. Turn it into a terminal failure so a queued replacement
        // can proceed instead of leaving the coordinator permanently active.
        let error = "scene load outcome arrived for a non-load transition".to_string();
        warn!("[scene] {error}");
        pending.outcome = None;
        commands.remove_resource::<SceneLoadInFlight>();
        commands.trigger(SceneTransitionFailed { transition, error });
        return;
    }

    if let SceneStageAssetOutcome::Failed { error, .. } = outcome {
        pending.outcome = None;
        commands.remove_resource::<SceneLoadInFlight>();
        commands.remove_resource::<lunco_usd_bevy::FailedSceneLoad>();
        commands.trigger(SceneTransitionFailed { transition, error });
        return;
    }

    let still_awaiting = q_awaiting
        .iter()
        .any(|prim| prim.stage_handle.id() == g.stage_id);
    if still_awaiting {
        // The asset is loaded, but visual projection is intentionally paced.
        // Keep the outcome until the queue has drained; this is a normal
        // multi-frame phase, not a lifecycle failure.
        return;
    }

    if let Some(contract) = camera_contract.as_deref() {
        if contract.required && !contract.ready {
            let detail = if contract.errors.is_empty() {
                "authored window presentation has not been validated".to_string()
            } else {
                contract.errors.join("; ")
            };
            let error = format!("scene `{}` failed camera contract: {detail}", g.path);
            error!("[scene] {error}");
            pending.outcome = None;
            commands.remove_resource::<SceneLoadInFlight>();
            commands.remove_resource::<lunco_usd_bevy::FailedSceneLoad>();
            commands.trigger(SceneTransitionFailed { transition, error });
            return;
        }
    }

    // A scene that is meant to be visible must provide its light through USD
    // (or the authored celestial bootstrap). Absence is reported, but it does
    // not change the transaction outcome.
    if q_lights.is_empty() {
        error!(
            "[scene] `{}` finished loading with no DirectionalLight — author a \\
             UsdLux DistantLight or a celestial site anchor",
            g.path
        );
    }
    pending.outcome = None;
    commands.remove_resource::<SceneLoadInFlight>();
    commands.remove_resource::<lunco_usd_bevy::FailedSceneLoad>();
    commands.trigger(SceneTransitionCompleted { transition });
}

pub(crate) fn process_usd_cosim_prims(
    mut commands: Commands,
    query: Query<(Entity, &UsdPrimPath), Without<UsdSourcedCosim>>,
    stages: Res<Assets<UsdStageAsset>>,
    // Read the LIVE canonical stage (source of truth), built on demand from
    // the asset's recipe.
    mut canonical: NonSendMut<CanonicalStages>,
    asset_server: Res<AssetServer>,
    mut wiring_dirty: ResMut<WiringDirty>,
    mut python_unavailable: ResMut<PythonUnavailablePrograms>,
) {
    // Which prims a component collection already owns, per stage. Computed once
    // per run rather than per prim (it is a full stage walk), and NOT cached
    // across runs: this system only runs while unprocessed prims remain, and
    // each prim is decided exactly once.
    let mut members_by_stage: HashMap<bevy::asset::AssetId<UsdStageAsset>, BTreeSet<String>> =
        HashMap::new();
    for (entity, prim_path) in query.iter() {
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else {
            continue;
        };

        // Acquire a read source: the live canonical stage, built on demand from
        // the asset recipe. If it is not available yet the asset is still
        // loading — retry next frame WITHOUT marking, so the prim stays in the
        // `Without<UsdSourcedCosim>` query.
        let id = prim_path.stage_handle.id();
        if canonical.get(id).is_none() {
            if let Some(recipe) = stages
                .get(&prim_path.stage_handle)
                .and_then(|a| a.recipe.clone())
            {
                canonical.get_or_build(id, &recipe);
            }
        }

        // Mark examined up front so each prim is inspected exactly once.
        // Without this, every *non-cosim* prim (wheels, ground, ramps — the
        // bulk of the scene) failed the active-cosim gate below via the
        // early `continue` WITHOUT ever gaining `UsdSourcedCosim`, so it stayed
        // in the `Without<UsdSourcedCosim>` query forever — and this system
        // re-ran every frame, deep-cloning the whole stage per prim. That was
        // the dominant sandbox CPU cost (see scripts/perf/README.md).
        // Safe: every other `UsdSourcedCosim` consumer also requires a
        // `ModelicaModel` / `SimComponent` / `ScriptedModel` that a non-cosim
        // prim never gains, so marking it here matches nothing downstream.
        // No live stage (asset carries no recipe / build failed) yet — skip,
        // leaving the prim in the `Without<UsdSourcedCosim>` query to retry.
        let Some(cs) = canonical.get(id) else {
            continue;
        };
        // `try_insert` (not `.insert`): a `LoadScene` cleanup may despawn this
        // prim between this system's iterate and ApplyDeferred — the canonical
        // race is the moonbase autoload vs a first-run tutorial on web. `.insert`
        // routes through Bevy's panic error handler, which aborts wasm; `try_insert`
        // silently drops the write on a despawned entity. Every entity-tied insert
        // queued by this pipeline uses the same despawn-safe form for the same
        // reason. See `lunco_usd_bevy::sync_usd_visuals` for the policy.
        commands.entity(entity).try_insert(UsdSourcedCosim);
        let view = cs.view();
        if view.has_api_schema(&sdf_path, "LunCoEnvironmentProbeAPI") {
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
            lunco_usd_bevy::program::modelica_network_member_paths(&view)
                .into_iter()
                .collect()
        });
        process_usd_cosim_prim_read(
            &view,
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
    unprocessed: Query<(), (With<UsdPrimPath>, Without<UsdSourcedCosim>)>,
) {
    if diagnostics.reported
        || diagnostics.paths.is_empty()
        || in_flight.is_some()
        || !unprocessed.is_empty()
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
    view: &lunco_usd_bevy::StageView<'_>,
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
    view: &lunco_usd_bevy::StageView<'_>,
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
        Has<lunco_core::PortSurfaceReady>,
        Has<lunco_core::PortSurfacePending>,
    )>,
    pending_interface_query: Query<(), (With<UsdSourcedCosim>, Without<SimComponent>)>,
    pending_query: Query<
        (
            Entity,
            &UsdPrimPath,
            Option<&lunco_core::Provenance>,
            Option<&lunco_core::GlobalEntityId>,
            Has<UsdInstanceRoot>,
        ),
        Without<UsdTelemetryProjected>,
    >,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<CanonicalStages>,
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

    for (entity, prim_path, provenance, gid, is_root) in &pending_query {
        let Some(recipe) = stages
            .get(&prim_path.stage_handle)
            .and_then(|asset| asset.recipe.clone())
        else {
            continue;
        };
        let id = prim_path.stage_handle.id();
        if canonical.get(id).is_none() {
            canonical.get_or_build(id, &recipe);
        }
        let Some(stage) = canonical.get(id) else {
            continue;
        };
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
        let view = stage.view();
        let authored = match read_authored_bool_strict(&view, &path, "lunco:telemetry") {
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
            let target_paths = view.rel_targets(&path, "lunco:telemetry:target");
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
                let port = read_authored_telemetry_string(&view, &path, "lunco:telemetry:port")?
                    .filter(|value| !value.is_empty());
                let reflect =
                    read_authored_telemetry_string(&view, &path, "lunco:telemetry:reflect")?
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
                let name = read_authored_telemetry_string(&view, &path, "lunco:telemetry:name")?
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| source_name.to_string());
                let display_name = view
                    .text(&path, "ui:displayName")
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| value.trim().to_owned());
                let unit = read_authored_telemetry_string(&view, &path, "lunco:telemetry:unit")?
                    .unwrap_or_default();
                let description =
                    read_authored_telemetry_string(&view, &path, "lunco:telemetry:description")?;
                let rate_hz =
                    match read_authored_telemetry_real(&view, &path, "lunco:telemetry:rateHz")? {
                        None | Some(0.0) => None,
                        Some(value) if value > 0.0 => Some(value),
                        Some(_) => return Err(()),
                    };
                let enabled =
                    match read_authored_bool_strict(&view, &path, "lunco:telemetry:enabled") {
                        Ok(Some(value)) => value,
                        Ok(None) => true,
                        Err(_) => return Err(()),
                    };
                let deadband =
                    match read_authored_telemetry_real(&view, &path, "lunco:telemetry:deadband")? {
                        None | Some(0.0) => None,
                        Some(value) if value > 0.0 => Some(value),
                        Some(_) => return Err(()),
                    };
                let retention = match view.scalar::<i64>(&path, "lunco:telemetry:retention") {
                    Some(0) | None
                        if !view.has_authored_attribute(&path, "lunco:telemetry:retention") =>
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
/// from the live composed [`UsdRead`] surface.
fn process_usd_cosim_prim_read(
    reader: &lunco_usd_bevy::StageView<'_>,
    entity: Entity,
    prim_path: &UsdPrimPath,
    sdf_path: &SdfPath,
    // Every prim some `CollectionAPI:components` scope on this stage owns.
    network_members: &BTreeSet<String>,
    commands: &mut Commands,
    asset_server: &AssetServer,
    wiring_dirty: &mut WiringDirty,
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
            Some(Value::Token(value)) => match parse_event_severity(value.as_str()) {
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
        commands.entity(entity).try_insert(EventBinding {
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
    // generated model by `domain_projection`. Compiling it here as well would
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
    if has_acausal_connector(reader, sdf_path) {
        if has_connected_acausal_connector(reader, sdf_path) {
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
    // BehaviorTree sources remain with their own projections.
    let resolved = match lunco_usd_bevy::program::resolve_program(reader, sdf_path) {
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
            lunco_usd_bevy::program::ProgramBackend::Modelica,
            lunco_usd_bevy::program::ProgramSource::Asset(path),
        ) => (
            lunco_usd_bevy::program::ProgramBackend::Modelica,
            Some(path),
            None,
        ),
        (
            lunco_usd_bevy::program::ProgramBackend::Python,
            lunco_usd_bevy::program::ProgramSource::Asset(path),
        ) => (
            lunco_usd_bevy::program::ProgramBackend::Python,
            None,
            Some(path),
        ),
        // A program this crate does not solve (a Rhai script, a behavior tree,
        // or a built-in driver) is somebody else's to run.
        _ => return,
    };
    let has_ports = reader
        .attr_names(sdf_path)
        .iter()
        .any(|n| n.starts_with("inputs:") || n.starts_with("outputs:"));
    if !has_ports {
        let (inputs, outputs) = declared_interface(reader, sdf_path);
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
            lunco_core::NotPredictable,
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
        commands.trigger(lunco_core::TelemetryEvent {
            name: MODEL_CONFIGURATION_INVALID.into(),
            source: 0,
            severity: lunco_core::Severity::Error,
            data: lunco_core::TelemetryValue::String(reason),
            timestamp: 0.0,
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
            let authored = reader
                .attr_names(sdf_path)
                .iter()
                .any(|name| name == "lunco:program:communicationPeriod");
            lunco_modelica::resolve_communication_period_secs(
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
            let (inputs, outputs) = declared_interface(reader, sdf_path);
            let model_name = modelica_path
                .as_deref()
                .map_or_else(|| "Modelica".to_string(), |path| format!("Modelica:{path}"));
            commands.entity(entity).try_insert((
                UsdSimProcessed,
                lunco_core::NotPredictable,
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
            commands.trigger(lunco_core::TelemetryEvent {
                name: MODEL_CONFIGURATION_INVALID.into(),
                source: 0,
                severity: lunco_core::Severity::Error,
                data: lunco_core::TelemetryValue::String(reason),
                timestamp: 0.0,
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
        if get_python_status() != PythonStatus::Available {
            let reason =
                format!("Python runtime unavailable; cannot run `{asset_path}` in this binary");
            python_unavailable.paths.insert(prim_path.path.clone());
            let (inputs, outputs) = declared_interface(reader, sdf_path);
            commands.entity(entity).try_insert((
                UsdSimProcessed,
                lunco_core::NotPredictable,
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
            commands.trigger(lunco_core::TelemetryEvent {
                name: MODEL_DISPATCH_FAILED.into(),
                source: 0,
                severity: lunco_core::Severity::Error,
                data: lunco_core::TelemetryValue::String(reason),
                timestamp: 0.0,
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
    // semantic possession boundary and excludes the `Avatar` endpoint; authority
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
        .try_insert(lunco_core::NotPredictable);

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
    let (mut inputs, outputs) = declared_interface(reader, sdf_path);
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
        });
    } else if let Some(asset_path) = python_path {
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
                .try_insert(lunco_cosim::RealtimeSafe);
        }
        Ok(Some(false)) | Ok(None) => {}
        Err(_) => warn!(
            "[usd-cosim] program {} has malformed `lunco:program:realtimeSafe`; promise ignored",
            prim_path.path
        ),
    }

    info!("[usd-cosim] program {} bound ({backend:?})", prim_path.path);
}

/// A `connectors:*` property declares an acausal Modelica interface. Such a
/// program is only executable as a member of a component network.
fn has_acausal_connector(reader: &lunco_usd_bevy::StageView<'_>, sdf_path: &SdfPath) -> bool {
    reader
        .attr_names(sdf_path)
        .iter()
        .any(|name| name.starts_with("connectors:"))
}

/// A bare `connectors:*` property declares an interface; only its connection
/// list makes an authoring claim about circuit topology.
fn has_connected_acausal_connector(
    reader: &lunco_usd_bevy::StageView<'_>,
    sdf_path: &SdfPath,
) -> bool {
    reader.attr_names(sdf_path).iter().any(|name| {
        name.starts_with("connectors:") && !reader.connections(sdf_path, name).is_empty()
    })
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
    mut notices: MessageWriter<lunco_modelica::ModelicaNotice>,
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
            notices.write(lunco_modelica::ModelicaNotice {
                level: lunco_modelica::NoticeLevel::Error,
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
    )>,
    sources: Res<Assets<ModelicaSource>>,
    asset_server: Res<AssetServer>,
    channels: Option<Res<ModelicaChannels>>,
    mut notices: MessageWriter<lunco_modelica::ModelicaNotice>,
    // The solver-selection input only carries the authored prediction contract.
    // Solver capability and Modelica lowering remain owned by the worker's
    // backend registry; they are never inferred from a DAE shape here.
    q_realtime_safe: Query<&lunco_cosim::RealtimeSafe>,
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
    pending.sort_unstable_by(|(_, _, a, _, _, _), (_, _, b, _, _, _)| a.path.cmp(&b.path));

    for (entity, pending, prim_path, mut component, usd_defaults, schedule) in pending {
        // Bail loud if the asset failed to load — without this the
        // entity stays Pending forever and the user sees nothing.
        if asset_server.load_state(&pending.handle).is_failed() {
            let error = format!(
                "failed to load Modelica source `{}` via AssetServer",
                pending.asset_path
            );
            warn!("[usd-cosim] {error}");
            notices.write(lunco_modelica::ModelicaNotice {
                level: lunco_modelica::NoticeLevel::Error,
                text: format!("[{}] Asset load error: {error}", component.model_name),
            });
            component.status = SimStatus::Error(error);
            commands
                .entity(entity)
                .try_remove::<PendingModelicaSource>();
            continue;
        }
        let Some(src) = sources.get(&pending.handle) else {
            continue;
        };

        // ONE parse-and-extract, shared with the network projector
        // (`lunco_modelica::parse_model_interface`): `ModelicaModel::inputs` is a
        // write buffer seeded from the authored interface, which
        // `wrap_modelica_into_simcomponent` copies into `SimComponent::inputs` —
        // the port surface a wire writes to.
        let interface = parse_model_interface(&src.text, "cosim-dispatch.mo");
        let model_name = interface.model_name.unwrap_or_else(|| "Model".into());
        let mut parameters = interface.parameters;
        let mut inputs = interface.inputs;
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
                        prim_path.path,
                        name,
                        model_name,
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
        // `source_roots::ensure_loaded` uses.
        let Some(schedule) = schedule else {
            let error = format!(
                "Modelica source `{}` has no projected co-simulation schedule",
                pending.asset_path
            );
            component.status = SimStatus::Error(error.clone());
            commands
                .entity(entity)
                .try_remove::<PendingModelicaSource>();
            error!("[usd-cosim] {error}");
            notices.write(lunco_modelica::ModelicaNotice {
                level: lunco_modelica::NoticeLevel::Error,
                text: format!("[{}] {error}", component.model_name),
            });
            commands.trigger(lunco_core::TelemetryEvent {
                name: MODEL_CONFIGURATION_INVALID.into(),
                source: 0,
                severity: lunco_core::Severity::Error,
                data: lunco_core::TelemetryValue::String(error),
                timestamp: 0.0,
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
        let dispatch_error = channels
            .tx
            .send(ModelicaCommand::Compile {
                entity,
                session_id: 0,
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
            resume_after_compile: dispatch_error.is_none(),
            ..default()
        });

        if let Some(error) = dispatch_error {
            error!("[usd-cosim] {error}");
            notices.write(lunco_modelica::ModelicaNotice {
                level: lunco_modelica::NoticeLevel::Error,
                text: format!("[{model_name}] {error}"),
            });
            // Immediate verdict for this tick; `modelica_status` keeps it from
            // the following one.
            component.status = SimStatus::Error(error.clone());
            commands.trigger(lunco_core::TelemetryEvent {
                name: MODEL_DISPATCH_FAILED.into(),
                source: 0,
                severity: lunco_core::Severity::Error,
                data: lunco_core::TelemetryValue::String(error),
                timestamp: 0.0,
            });
        }

        commands
            .entity(entity)
            .try_remove::<PendingModelicaSource>();
    }
}

/// Drain `PendingPythonSource` analogously to the Modelica version.
fn python_source_load_error(asset_path: &str) -> String {
    format!("failed to load Python source `{asset_path}` via AssetServer")
}

fn mark_python_source_load_failed(sim: &mut SimComponent, error: &str) {
    // The bind-time interface is still valid, but the executable source is not.
    // This terminal state releases the binding epoch and readiness producer;
    // leaving `Compiling` here would wait forever for a source that cannot arrive.
    sim.status = SimStatus::Error(error.to_owned());
}

pub fn dispatch_loaded_python_sources(
    mut commands: Commands,
    q: Query<(Entity, &PendingPythonSource)>,
    sources: Res<Assets<PythonSource>>,
    asset_server: Res<AssetServer>,
    mut registry: ResMut<ScriptRegistry>,
    mut notices: MessageWriter<lunco_modelica::ModelicaNotice>,
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
            notices.write(lunco_modelica::ModelicaNotice {
                level: lunco_modelica::NoticeLevel::Error,
                text: format!("[{model_name}] Asset load error: {error}"),
            });
            commands.trigger(lunco_core::TelemetryEvent {
                name: MODEL_DISPATCH_FAILED.into(),
                source: 0,
                severity: lunco_core::Severity::Error,
                data: lunco_core::TelemetryValue::String(error),
                timestamp: 0.0,
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

        // Offset the Python document id away from any Modelica-allocated ids
        // on the same entity.
        let doc_id = DocumentId::new(entity.index().index() as u64 + 10_000);
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
                params: String::new(),
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
                paused: false,
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
) {
    for (entity, model, contract) in q_new.iter() {
        let mut entity_commands = commands.entity(entity);
        entity_commands.try_insert(SimComponent {
            model_name: model.model_name.clone(),
            parameters: model.parameters.clone(),
            inputs: model.inputs.clone(),
            // Outputs are the SOLUTION — empty until the worker answers.
            // `sync_modelica_outputs` fills them and flips the status.
            outputs: model.variables.clone(),
            status: modelica_status(model),
            is_stepping: model.is_stepping,
        });
        // Generated domain roots publish their ModelicaModel one Update before
        // this wrapper can expose the shared SimComponent port surface. Keep
        // telemetry and connection diagnostics in typed assembly-pending state
        // for that real lifecycle interval; the wrapper owns readiness once it
        // has been inserted.
        entity_commands.remove::<lunco_core::PortSurfacePending>();
        if let Some(contract) = contract {
            entity_commands.try_insert(DeclaredOutputPorts {
                names: contract.outputs.iter().cloned().collect(),
            });
        }
    }
}

/// The status a `ModelicaModel` projects onto its `SimComponent`.
///
/// A durable worker error wins. Compilation also is not readiness: its result
/// contains an initial algebraic snapshot, but no live inputs have reached the
/// solver yet. A model becomes `Running` only after its first successful solver
/// advance, so load readiness cannot release physics onto zero-valued,
/// compile-time ports.
/// One place keeps bind-time insertion and per-tick sync consistent.
fn modelica_status(model: &ModelicaModel) -> SimStatus {
    if let Some(error) = &model.last_error {
        SimStatus::Error(error.clone())
    } else if !model.is_compiled || model.current_time <= 0.0 {
        SimStatus::Compiling
    } else if model.paused {
        SimStatus::Paused
    } else {
        SimStatus::Running
    }
}

/// Copy `f64` port values into a destination map, allocating a `String`
/// key only on the *first* tick a port appears. The cosim sync systems
/// below run every `FixedUpdate`; the keys (`"height"`, `"netForce"`, …)
/// are stable, so after the first step every port already exists and
/// this updates in place with zero allocation. The old
/// `dst.insert(name.clone(), v)` re-allocated every key every tick.
#[inline]
fn upsert_ports<'a>(
    dst: &mut HashMap<String, f64>,
    src: impl Iterator<Item = (&'a String, &'a f64)>,
) {
    for (name, val) in src {
        match dst.get_mut(name) {
            Some(slot) => *slot = *val,
            None => {
                dst.insert(name.clone(), *val);
            }
        }
    }
}

/// Per-tick: ModelicaModel.variables → SimComponent.outputs.
/// Lets `propagate_connections` see fresh Modelica outputs each step.
pub fn sync_modelica_outputs(
    mut q: Query<(&ModelicaModel, &mut SimComponent), With<UsdSourcedCosim>>,
) {
    for (model, mut comp) in &mut q {
        upsert_ports(&mut comp.outputs, model.variables.iter());
        for (k, v) in &model.inputs {
            comp.inputs.entry(k.clone()).or_insert(*v);
        }
        comp.status = modelica_status(model);
    }
}

/// Copy only values accepted by the Modelica program into its solver input
/// buffer. A USD program prim can also carry physical sink attributes on the
/// same entity (`inputs:force_y` for Avian); those names belong to the shared
/// cosim surface but are not Modelica inputs. Promoting them into
/// `ModelicaModel::inputs` would hide same-named Modelica actuator outputs from
/// the worker's observable set and stop the self-loop from ever binding.
fn copy_modelica_input_values(
    model: &mut ModelicaModel,
    component: &SimComponent,
    command_surface: Option<&lunco_core::InputPorts>,
) {
    for (name, value) in &component.inputs {
        if model.inputs.contains_key(name) || model.compiled_input_names.contains(name) {
            model.inputs.insert(name.clone(), *value);
        }
    }

    // `InputPorts` is the authored command surface and is registered before the
    // Modelica backend. When a generated domain root carries both components,
    // SetPorts therefore writes the command there; mirror that authoritative
    // value into the solver buffer instead of leaving Modelica on its bind-time
    // default. This is the generic bridge for a shared endpoint, not a vehicle
    // or tutorial-specific path.
    if let Some(command_surface) = command_surface {
        for (name, value) in &command_surface.values {
            if model.inputs.contains_key(name) || model.compiled_input_names.contains(name) {
                model.inputs.insert(name.clone(), *value);
            }
        }
    }
}

/// Per-tick: shared command/wire inputs → ModelicaModel.inputs.
/// Hands wire-propagated values (height, velocity, …) back to the
/// Modelica worker for the next solver step. An authored `InputPorts` command
/// surface wins for names it owns because it is the public control boundary;
/// the `SimComponent` map remains the destination for propagated model inputs.
pub fn sync_modelica_inputs(
    mut q: Query<
        (
            &SimComponent,
            Option<&lunco_core::InputPorts>,
            &mut ModelicaModel,
        ),
        With<UsdSourcedCosim>,
    >,
) {
    for (comp, command_surface, mut model) in &mut q {
        copy_modelica_input_values(&mut model, comp, command_surface);
    }
}

/// Per-tick: ScriptedModel.outputs → SimComponent.outputs.
pub fn sync_script_outputs(
    mut q: Query<(&ScriptedModel, &mut SimComponent), With<UsdSourcedCosim>>,
) {
    for (model, mut comp) in &mut q {
        upsert_ports(&mut comp.outputs, model.outputs.iter());
    }
}

/// Per-tick: SimComponent.inputs → ScriptedModel.inputs.
pub fn sync_script_inputs(
    mut q: Query<(&SimComponent, &mut ScriptedModel), With<UsdSourcedCosim>>,
) {
    for (comp, mut model) in &mut q {
        upsert_ports(&mut model.inputs, comp.inputs.iter());
    }
}

// ── Connected discrete signal → event bus ───────────────────────────────────

/// Runtime projection of one authored `LunCoEvent`.
#[derive(Component)]
pub struct EventBinding {
    source_path: String,
    output: String,
    name: String,
    severity: lunco_core::Severity,
    latched: bool,
    qualification_time_s: f64,
    qualified_for_s: f64,
    armed: bool,
}

fn parse_event_severity(value: &str) -> Option<lunco_core::Severity> {
    match value {
        "debug" => Some(lunco_core::Severity::Debug),
        "info" => Some(lunco_core::Severity::Info),
        "warning" => Some(lunco_core::Severity::Warning),
        "error" => Some(lunco_core::Severity::Error),
        "critical" => Some(lunco_core::Severity::Critical),
        _ => None,
    }
}

fn event_rising_edge(
    armed: &mut bool,
    qualified_for_s: &mut f64,
    qualification_time_s: f64,
    latched: bool,
    value: f64,
    delta_s: f64,
) -> bool {
    let active = value >= 0.5;
    if !active {
        *qualified_for_s = 0.0;
        if !latched {
            *armed = true;
        }
        return false;
    }
    if !*armed {
        return false;
    }
    *qualified_for_s += delta_s.max(0.0);
    if *qualified_for_s >= qualification_time_s.max(0.0) {
        *armed = false;
        true
    } else {
        false
    }
}

/// Project rising edges of connected 0/1 model outputs onto [`TelemetryEvent`].
pub fn fire_connected_events(
    mut bindings: Query<(Entity, &UsdPrimPath, &mut EventBinding)>,
    sources: Query<(
        Entity,
        &UsdPrimPath,
        &SimComponent,
        Option<&lunco_core::GlobalEntityId>,
    )>,
    q_gid: Query<&lunco_core::GlobalEntityId>,
    q_provenance: Query<&lunco_core::Provenance>,
    q_instance_root: Query<(), With<UsdInstanceRoot>>,
    fixed_time: Res<Time<Fixed>>,
    world_time: Option<Res<lunco_time::WorldTime>>,
    mut commands: Commands,
) {
    // Nothing is listening: don't index the scene. This runs every FixedUpdate
    // tick and the index below is a full scan of every cosim participant plus a
    // fresh allocation — paid on every scene, while `LunCoEvent` prims are rare.
    if bindings.is_empty() {
        return;
    }
    let Some(world_time) = world_time else {
        warn!("[usd-cosim] cannot publish a connected event without the authoritative WorldTime");
        return;
    };
    let instance_of =
        |entity| lunco_usd_bevy::instance_key(entity, &q_provenance, &q_gid, &q_instance_root);
    let mut by_path = HashMap::new();
    for (entity, path, component, gid) in &sources {
        by_path.insert(
            (instance_of(entity), path.path.as_str()),
            (component, gid.map(|id| id.get()).unwrap_or(0)),
        );
    }
    for (entity, _, mut binding) in &mut bindings {
        let Some((value, source)) = by_path
            .get(&(instance_of(entity), binding.source_path.as_str()))
            .and_then(|(component, source)| {
                component
                    .outputs
                    .get(&binding.output)
                    .map(|value| (*value, *source))
            })
        else {
            continue;
        };
        let latched = binding.latched;
        let qualification_time_s = binding.qualification_time_s;
        let delta_s = fixed_time.delta_secs_f64();
        let EventBinding {
            armed,
            qualified_for_s,
            ..
        } = &mut *binding;
        if event_rising_edge(
            armed,
            qualified_for_s,
            qualification_time_s,
            latched,
            value,
            delta_s,
        ) {
            commands.trigger(lunco_core::TelemetryEvent {
                name: binding.name.clone(),
                source,
                severity: binding.severity,
                data: lunco_core::TelemetryValue::F64(value),
                timestamp: world_time.epoch_jd,
            });
        }
    }
}

/// Marker on a [`SimConnection`] that was **derived** from USD `connectionPaths`
/// (as opposed to authored some other way). [`rewire_usd_connections`] despawns
/// every tagged edge and rebuilds the set from the composed stage, which is what
/// makes `SimConnection` a **pure derived cache** of USD wiring.
#[derive(Component, Default)]
pub struct UsdWiredConnection;

/// Set when a drained live edit — a journaled (hence distributed)
/// `connectionPaths` change on an **already-spawned** prim — requires the wiring
/// to be re-derived. Structural changes (prim spawn/despawn) need no flag; they
/// are detected directly via change-detection in [`rewire_usd_connections`].
#[derive(Resource, Default)]
pub struct WiringDirty(pub bool);

/// Run condition for the derived USD wiring cache.
///
/// Keep the expensive composed-stage sweep out of stable frames. The system
/// itself retains the same guard for direct minimal-app use and for tests; the
/// production plugin uses this condition so Bevy does not enter that system at
/// all until an endpoint, identity, authority, removal, or live edit arrives.
fn wiring_due(
    arrivals: Query<
        (),
        Or<(
            Added<UsdPrimPath>,
            Added<ModelicaModel>,
            Added<SimComponent>,
            Added<lunco_core::GlobalEntityId>,
            Added<lunco_core::PortSurface>,
            Added<lunco_core::PortSurfaceReady>,
        )>,
    >,
    mut removed: RemovedComponents<UsdPrimPath>,
    dirty: Res<WiringDirty>,
    role: Option<Res<lunco_core::NetworkRole>>,
) -> bool {
    !arrivals.is_empty()
        || removed.read().next().is_some()
        || dirty.0
        || role.is_some_and(|role| role.is_changed())
}

/// Coalesces endpoint lifecycle events into one settlement decision. It is not
/// a timer: observers and Modelica change detection are its only writers.
#[derive(Resource, Default)]
pub(crate) struct BindingEpochDirty(pub bool);

/// Last published Modelica participant status. `SimComponent` also carries
/// continuously changing inputs/outputs, so Bevy's broad `Changed<SimComponent>`
/// signal is not by itself a binding-lifecycle event.
#[derive(Resource, Default)]
struct BindingModelStatuses(HashMap<Entity, SimStatus>);

#[derive(Resource)]
pub(crate) struct BindingEpochWait(pub(crate) lunco_readiness::ReadinessTicket);

fn request_binding_epoch<T: Component>(_trigger: On<Add, T>, mut dirty: ResMut<BindingEpochDirty>) {
    dirty.0 = true;
}

fn request_binding_epoch_on_remove<T: Component>(
    _trigger: On<Remove, T>,
    mut dirty: ResMut<BindingEpochDirty>,
) {
    dirty.0 = true;
}

fn request_binding_epoch_on_model_change(
    changed: Query<(Entity, &SimComponent), Changed<SimComponent>>,
    mut statuses: ResMut<BindingModelStatuses>,
    mut dirty: ResMut<BindingEpochDirty>,
) {
    for (entity, component) in &changed {
        if statuses
            .0
            .get(&entity)
            .is_none_or(|previous| previous != &component.status)
        {
            statuses.0.insert(entity, component.status.clone());
            dirty.0 = true;
        }
    }
}

fn forget_binding_model_status(
    trigger: On<Remove, SimComponent>,
    mut statuses: ResMut<BindingModelStatuses>,
    mut dirty: ResMut<BindingEpochDirty>,
) {
    statuses.0.remove(&trigger.entity);
    dirty.0 = true;
}

fn modelica_models_terminal<'a>(
    mut models: impl Iterator<Item = (Option<&'a ModelicaModel>, Option<&'a SimComponent>)>,
) -> bool {
    models.all(|(model, component)| match (model, component) {
        // A bind-published SimComponent with no ModelicaModel is the async
        // source-load gap. Its status is deliberately Compiling, so it must
        // not seal the epoch before dispatch has created the authoritative
        // solver participant.
        (None, Some(component)) => !matches!(component.status, SimStatus::Compiling),
        (None, None) => true,
        // `SimComponent::status` intentionally remains `Compiling` until the
        // first solver step has produced live outputs.  That is the public
        // simulation status, not the source-compilation transaction.  Once
        // the authoritative Modelica worker has compiled the model, the
        // binding epoch must be allowed to seal; otherwise scene readiness is
        // coupled to the first fixed tick and a cold compile can hold the
        // world indefinitely.  The next fixed tick will promote the wrapper
        // to Running (or reopen the epoch if endpoint admission is still
        // pending).
        (Some(model), Some(component)) => {
            model.is_compiled
                || model.last_error.is_some()
                || matches!(component.status, SimStatus::Error(_))
        }
        (Some(_), None) => false,
    })
}

/// Reconcile the USD projection epoch with the native binding transaction.
/// Failed models are terminal: readiness policy decides whether to hold
/// physics, while the binder must be allowed to record the failed endpoint.
///
/// Native endpoint admission is deliberately *not* a world-level readiness
/// hold. `PendingUsdJoint`, `PendingWheelWiring`, and `PendingDifferential` are
/// local activation gates: the affected bodies remain kinematic until their
/// endpoint is ready. Their preparation runs in the fixed schedule and their
/// structural admission runs in the outer `Update` schedule, so globally
/// pausing Avian's nested `PhysicsSchedule` does not deadlock those markers.
/// A deferred USD stage is different: its projection can still add arbitrary
/// bodies and connections, so it retains the world hold until the stage is
/// available.
fn settle_binding_epoch(
    awaiting: Query<(), With<lunco_usd_bevy::UsdAwaitingStage>>,
    joints: Query<(), With<lunco_usd_avian::PendingUsdJoint>>,
    wheels: Query<(), With<crate::PendingWheelWiring>>,
    differentials: Query<(), With<crate::PendingDifferential>>,
    // `UsdSourcedCosim` marks the USD projection domain, not a solver.  It is
    // intentionally also present on native endpoints such as a revolute joint
    // so they can expose ports through the same scene surface.  A joint has no
    // `ModelicaModel` or `SimComponent` by design, and treating that absence as
    // a compiling model leaves the whole world held forever after its physical
    // admission.
    //
    // Solver readiness ranges over Modelica owners and their projected
    // SimComponents.  The optional component matters: the projection frame
    // between a ModelicaModel arriving and its SimComponent wrapper is itself
    // not terminal. Native endpoints have dedicated readiness facts above
    // (`PendingUsdJoint`, wheel wiring, and differential wiring), so omitting
    // them here does not weaken the binding transaction.
    models: Query<(Option<&ModelicaModel>, Option<&SimComponent>), With<UsdSourcedCosim>>,
    connections: Query<(), With<SimConnection>>,
    mut dirty: ResMut<BindingEpochDirty>,
    mut revision: ResMut<lunco_cosim::BindingRevision>,
    wait: Option<Res<BindingEpochWait>>,
    mut readiness: ResMut<lunco_readiness::ReadinessRegistry>,
    mut commands: Commands,
) {
    let models_terminal = modelica_models_terminal(models.iter());
    let settled = awaiting.is_empty()
        && joints.is_empty()
        && wheels.is_empty()
        && differentials.is_empty()
        && models_terminal;
    if settled {
        dirty.0 = false;
        revision.seal_epoch();
    } else {
        // Keep the reconciliation scheduled until every deferred stage,
        // joint, wheel, differential, and model participant has reached a
        // terminal state. Some of those transitions come from async asset or
        // compiler completion and do not emit one of the structural events
        // that originally opened this epoch.
        dirty.0 = true;
        revision.open_epoch();
    }
    // Seal/open is an event, not a condition the fixed-step master polls. The
    // single binding transaction runs in `lunco_cosim`'s `PostUpdate` boundary,
    // after every projection and endpoint-lifecycle update for this frame. Do
    // not queue it here: doing so let first-load USD ports and generated domain
    // contracts race each other across deferred command boundaries.
    // Modelica compilation has its own per-entity readiness tickets. Do not
    // turn the binding epoch into a world hold while one of those models is
    // cold-compiling; otherwise the world ticket waits on the same compiler
    // and defeats entity-scoped readiness. Native endpoint markers are also
    // local gates (see the function contract above), so only a deferred stage
    // warrants pausing the whole world here.
    let hold_binding_epoch =
        !settled && models_terminal && !connections.is_empty() && !awaiting.is_empty();
    match (hold_binding_epoch, wait) {
        (true, None) => {
            let ticket = readiness.begin(
                lunco_readiness::Subject::World,
                lunco_readiness::kinds::PARTICIPANT_INIT,
                "USD connection binding",
            );
            commands.insert_resource(BindingEpochWait(ticket));
        }
        (false, Some(wait)) => {
            readiness.finish(wait.0);
            commands.remove_resource::<BindingEpochWait>();
        }
        _ => {}
    }
}

/// Derive the co-sim wiring from native USD `connectionPaths`. `SimConnection`s
/// are a **pure derived cache**: whenever the wiring
/// topology may have changed, the whole derived set is rebuilt from the composed
/// stage. A full rebuild (not a per-prim patch) is what makes the lifecycle
/// correct — an edge exists exactly when *both* its endpoints do, regardless of
/// the order they spawn or which end is removed.
///
/// Trigger (dormant otherwise — steady state is zero work):
/// - **structural** — any `UsdPrimPath` entity added or removed, or a projected
///   [`ModelicaModel`] arriving on a domain root. Covers initial scene load (the
///   reconcile spawns prims → this fires), async payload/vessel spawn,
///   source-after-sink ordering, and a generated island publishing its boundary
///   contract (each re-runs this and completes the deferred edge); prim removal
///   omits its edge — no dangling `SimConnection`.
/// - **live edit** — [`WiringDirty`], set by the op-driven projection
///   ([`lunco_usd::live_consume`]) when a `connectionPaths` change is drained
///   from the live stage (an edit that is not itself a prim spawn/despawn).
///
/// A connection whose source prim is not yet spawned is skipped (its later spawn
/// re-runs this); a malformed source path is logged and skipped — restoring the
/// diagnostic the deleted `process_usd_cosim_wire_read` emitted.
/// `inputs:` ports whose connection is a STRUCTURAL binding — read once at PARSE to
/// discover topology — rather than a live scalar wire.
///
/// Each row is `(port, api_schema)`: the port name, and the applied schema that makes
/// the prim the kind of thing whose parse-time reader consumes it. Both halves matter —
/// `torque` is a perfectly ordinary live port on a motor, and only structural on a
/// gearbox.
///
/// A USD connection into a structural vehicle binding is real authoring, but the
/// FSW port registry owns that edge rather than the generic scalar binder. Every
/// other authored input is handled by the generic runtime endpoint surface.
const STRUCTURAL_INPUT_BINDINGS: &[(&str, &str)] = &[
    // `Wheel.inputs:drive` / `inputs:steer` when the authored source is a
    // vehicle control surface. Read by `connected_port` in `crate::lib`, which
    // resolves the authored connection to the FSW port NAME the wheel
    // subscribes to (`PendingWheelWiring`). A source from another authored
    // output, such as a generated domain member, is a normal scalar wire and
    // is intentionally not listed here.
    ("drive", "LunCoWheelAPI"),
    ("steer", "LunCoWheelAPI"),
];

/// Is this `inputs:` port one of [`STRUCTURAL_INPUT_BINDINGS`] on this prim?
fn is_structural_binding(view: &lunco_usd_bevy::StageView<'_>, sink: &SdfPath, port: &str) -> bool {
    STRUCTURAL_INPUT_BINDINGS
        .iter()
        .any(|(name, schema)| *name == port && view.has_api_schema(sink, schema))
}

pub fn rewire_usd_connections(
    mut commands: Commands,
    // Any endpoint identity or contract arriving must re-derive the USD wire
    // cache. Keeping the three arrival causes in one query avoids giving the
    // composition system parallel change-detection paths.
    wiring_arrivals: Query<
        (),
        Or<(
            Added<UsdPrimPath>,
            Added<ModelicaModel>,
            // Generated networks publish their actual port surface one
            // deferred step after `ModelicaModel` is installed. Re-run the
            // derived USD wiring when that endpoint contract arrives; without
            // this transition a boundary wire can be permanently absent while
            // diagnostics quite correctly report no broken edge.
            Added<SimComponent>,
            Added<lunco_core::GlobalEntityId>,
            // A generic physical surface can be installed after a broader
            // endpoint marker (for example a rigid body) already exists. The
            // surface itself is the authoritative transition for its named
            // ports; do not rely on the earlier marker to trigger a rebuild.
            Added<lunco_core::PortSurface>,
            Added<lunco_core::PortSurfaceReady>,
        )>,
    >,
    mut removed: RemovedComponents<UsdPrimPath>,
    mut dirty: ResMut<WiringDirty>,
    // Wiring consumes a projected endpoint, not an initial path stub. A prim
    // path is not itself an endpoint: require the generic port-surface marker
    // or a declared SimComponent interface before indexing it. The visual-sync
    // marker is intentionally not part of this contract: a standard USD light
    // publishes its scene-property surface when the renderer installs the
    // Bevy light, and that surface is a valid sink even if visual bookkeeping
    // is scheduled in a different deferred command batch.
    q_all: Query<
        (
            Entity,
            &UsdPrimPath,
            Has<ModelicaModel>,
            Option<&GeneratedModelicaSource>,
            Has<lunco_environment::EnvironmentProbe>,
            Option<&lunco_core::PortSurface>,
        ),
        Or<(
            With<lunco_core::PortSurfaceReady>,
            With<lunco_core::PortSurface>,
            With<SimComponent>,
        )>,
    >,
    q_edges: Query<Entity, With<UsdWiredConnection>>,
    // Wire endpoints resolve by IDENTITY, not raw prim path. Two runtime spawns of
    // the same asset compose byte-IDENTICAL stage-relative paths (`/DescentLander`,
    // …), so a flat path→entity map collapses them onto one entity — a lander's
    // force self-loop would then bind to the OTHER lander's model and both bodies
    // move as one. A prim's *instance* is named by its instance-root `GlobalEntityId`:
    // `Provenance::Derived{parent}` for a descendant, the root's own GID for the
    // instance root. That id is unique per spawn, identical on every peer, and
    // STABLE across a program/script hot-swap (it is `derive_id(parent, role)`, a
    // pure function of identity, not of the ephemeral `Entity`) — so a wire re-
    // resolves to the same endpoints after a dynamic script change.
    q_gid: Query<&lunco_core::GlobalEntityId>,
    q_provenance: Query<&lunco_core::Provenance>,
    q_instance_root: Query<(), With<UsdInstanceRoot>>,
    // The realtime gate: whether the SOURCE program promised it is realtime-safe,
    // and whether the SINK is a client-predicted dynamic body (a `RigidBody` NOT
    // opted out of prediction). The network role is part of this contract: only
    // a pure client predicts the body locally. Standalone and host processes are
    // authoritative, so their live solver is not incorrectly classified as a
    // prediction loop.
    role: Option<Res<lunco_core::NetworkRole>>,
    q_realtime_safe: Query<&lunco_cosim::RealtimeSafe>,
    q_predicted_body: Query<&avian3d::prelude::RigidBody, Without<lunco_core::NotPredictable>>,
    q_defaults: Query<&UsdInputDefaults>,
    // A producer's output ports are child `Port` entities, so an `outputs:`
    // forward onto one has to write there, not onto the producer prim.
    q_outputs: Query<&lunco_core::OutputPorts>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<CanonicalStages>,
) {
    let client_predicts = matches!(role.as_deref(), Some(lunco_core::NetworkRole::Client));
    let role_changed = role.as_ref().is_some_and(|role| role.is_changed());

    // `Added<ModelicaModel>` is the explicit endpoint-contract transition. This
    // pass no longer relies on accidentally deferred removal events to get an
    // extra rewire after a generated model appears.
    let structural = !wiring_arrivals.is_empty()
        || removed.read().next().is_some()
        // Changing authority changes whether a force edge is admissible. Rebuild
        // immediately on a standalone/host ↔ client transition instead of
        // leaving the previous role's wiring decision cached.
        || role_changed;
    if !structural && !dirty.0 {
        return;
    }
    dirty.0 = false;

    // A prim's instance identity (its instance-root GID, `None` for scene prims)
    // is what keeps two spawns of one asset — byte-identical stage-relative paths
    // and all — from collapsing onto one entity below. See `instance_key`.
    let instance_of =
        |e: Entity| lunco_usd_bevy::instance_key(e, &q_provenance, &q_gid, &q_instance_root);

    // Index every prim entity by (stage, instance, path). The stage is part of
    // prim identity: two composed USD projections may carry the same path text
    // while belonging to different stage assets. Omitting it lets a later
    // projection silently overwrite the first one, which can bind a simulation
    // wire to a transform-only entity instead of the light/material/physics
    // projection that owns the named port.
    //
    // The instance key still keeps two runtime spawns of one stage distinct;
    // the stage key keeps independently composed stages distinct.
    let mut by_path: HashMap<(bevy::asset::AssetId<UsdStageAsset>, Option<u64>, String), Entity> =
        HashMap::new();
    // A generated network is one Modelica participant, while its composed
    // member paths remain valid USD addresses for presentation and external
    // scalar consumers. This table translates those member output addresses to
    // the generated wrapper output declared by the projection. It is derived
    // from generated source metadata, not from any vehicle, sensor, or renderer
    // type, so every generated domain gets the same boundary behavior.
    let mut generated_member_outputs: HashMap<
        (
            bevy::asset::AssetId<UsdStageAsset>,
            Option<u64>,
            String,
            String,
        ),
        (Entity, String),
    > = HashMap::new();
    let environment_probe_entities: std::collections::HashSet<Entity> = q_all
        .iter()
        .filter_map(|(entity, _, _, _, is_probe, _)| is_probe.then_some(entity))
        .collect();
    let port_surfaces: HashMap<Entity, lunco_core::PortSurface> = q_all
        .iter()
        .filter_map(|(entity, _, _, _, _, surface)| {
            surface.cloned().map(|surface| (entity, surface))
        })
        .collect();
    for (e, p, _, generated, _, _) in q_all.iter() {
        let instance = instance_of(e);
        let key = (p.stage_handle.id(), instance, p.path.clone());
        by_path.insert(key, e);
        if let Some(generated) = generated {
            for (member, output, alias) in &generated.member_output_aliases {
                generated_member_outputs.insert(
                    (
                        p.stage_handle.id(),
                        instance,
                        member.clone(),
                        output.clone(),
                    ),
                    (e, alias.clone()),
                );
            }
        }
    }

    // Authored constants on unconnected `inputs:` ports — a model's parameters.
    // Gathered in the same sweep that derives the wires, because "has no wire" is
    // exactly what makes an input a parameter.
    let mut defaults: HashMap<Entity, HashMap<String, f64>> = HashMap::new();

    // Earth demand is a composed-wire fact, not a property of every environment
    // probe. Rebuild the projection from the same connection sweep below so a
    // live wire edit removes demand as well as adding it.
    for entity in &environment_probe_entities {
        commands
            .entity(*entity)
            .remove::<lunco_environment::EarthDirectionRequired>();
    }
    let mut earth_direction_required = std::collections::HashSet::new();

    // Network membership, per stage — see the skip below. One stage walk per
    // rebuild, not per prim.
    let mut members_by_stage: HashMap<
        bevy::asset::AssetId<UsdStageAsset>,
        std::collections::HashSet<String>,
    > = HashMap::new();

    // Rebuild: drop every derived edge, then re-derive from the composed stage.
    for e in q_edges.iter() {
        commands.entity(e).try_despawn();
    }

    for (entity, prim_path, has_modelica, _, _, wheel_endpoints) in q_all.iter() {
        let id = prim_path.stage_handle.id();
        if canonical.get(id).is_none() {
            if let Some(recipe) = stages
                .get(&prim_path.stage_handle)
                .and_then(|a| a.recipe.clone())
            {
                canonical.get_or_build(id, &recipe);
            }
        }
        let Some(cs) = canonical.get(id) else {
            continue;
        };
        let view = cs.view();
        let Ok(sink_sdf) = SdfPath::new(&prim_path.path) else {
            continue;
        };
        // `LunCoEvent.inputs:trigger` is a standard USD connection, but its
        // consumer is the event projector below rather than the scalar
        // SimConnection fabric.
        if view.type_name(&sink_sdf).as_deref() == Some("LunCoEvent") {
            continue;
        }
        // A component inside a synthesized Modelica network: its causal AND
        // acausal edges are compiled into the wrapper, so only the containing
        // Scope participates in scalar runtime propagation. A wire built here
        // would target a member that has no `SimComponent` of its own —
        // a phantom edge that can never land.
        //
        // MEMBERSHIP decides it, the same test `process_usd_cosim_prims` uses
        // for who owns a member's solver. It used to be "declares
        // `connectors:*`", which reads the same only because every shipped
        // member has a pin: a causal-only member kept its `inputs:` wired at
        // runtime as well as compiled into the wrapper, so the equation and the
        // wire both drove it.
        let members = members_by_stage
            .entry(id)
            .or_insert_with(|| lunco_usd_bevy::program::modelica_network_member_paths(&view));
        if members.contains(&prim_path.path) {
            continue;
        }

        // Resolve this prim's wires within its OWN instance — a source path names a
        // prim of the same spawn, never a same-named prim of a different one.
        let sink_instance = instance_of(entity);

        for attr in view.attr_names(&sink_sdf) {
            // An `outputs:X.connect` is a FORWARD: this prim publishes an interior
            // node's result as its own X. It is how a component REPLACES a producer
            // — a Modelica drive law supplies the vessel's `drive_left`, and not one
            // consumer of that port moves.
            //
            // Materialised as an ordinary edge whose SINK is X's own storage on this
            // prim, so every existing reader keeps reading the port it always read.
            // A vessel's actuator ports live on child `Port` entities
            // (`OutputPorts`, one `value` scalar each), which is where the write
            // has to land; anything else writes the name on the prim itself.
            //
            // One hop per authored forward, so a chain resolves as a chain of edges
            // — no walk, and no second resolution path for consumers to disagree
            // about. This is the only reader of output connections: before it,
            // `outputs:*.connect` was authored in three drive-law overlays and in
            // the rover network root in `skid_rover.usda` and did nothing at all, which is
            // why the Modelica rover travelled 0.00 m against a control at 2.12 m/s.
            // `outputs:` is UsdShade's namespace too. A Material's `outputs:surface`
            // connects to a Shader terminal — a shading-network edge, not a scalar
            // port — and materialising it as a wire targets a `surface` port nothing
            // will ever claim. Same reasoning as the `outputs:` filter in the vessel
            // actuator-port scan, which drops non-numeric attributes for this exact
            // reason.
            let shading_prim = matches!(
                view.type_name(&sink_sdf).as_deref(),
                Some("Material" | "Shader" | "NodeGraph")
            );
            let forward = attr
                .strip_prefix("outputs:")
                .filter(|_| !shading_prim)
                .map(|name| {
                    match q_outputs
                        .get(entity)
                        .ok()
                        .and_then(|outputs| outputs.get(name))
                    {
                        Some(port_entity) => (port_entity, lunco_cosim::PORT_NAME.to_string()),
                        None => (entity, name.to_string()),
                    }
                });
            // `inputs:` is a sink; `outputs:` is a sink only when it forwards.
            // Everything else on the prim is not part of the wire fabric.
            let Some(sink_conn) = attr
                .strip_prefix("inputs:")
                .or_else(|| attr.strip_prefix("outputs:").filter(|_| forward.is_some()))
            else {
                continue;
            };
            // `connectionPaths` belong to the USD property named
            // `inputs:<port>.connect`; `.connect` is metadata on that property,
            // never part of the simulation port's name.  Keep the raw `attr`
            // for the stage lookup below, but use this canonical connector name
            // for every runtime decision and edge endpoint.
            let sink_conn = sink_conn.strip_suffix(".connect").unwrap_or(sink_conn);
            // A structural binding is already resolved; building a phantom wire
            // for it manufactures a dangling-wire report that can never clear.
            let wheel_source_is_structural = if matches!(sink_conn, "drive" | "steer")
                && view.has_api_schema(&sink_sdf, "LunCoWheelAPI")
            {
                view.connections(&sink_sdf, &attr).iter().all(|source| {
                    let Some((source_prim, _)) = source.rsplit_once('.') else {
                        return false;
                    };
                    let Ok(source_path) = SdfPath::new(source_prim) else {
                        return false;
                    };
                    view.has_api_schema(&source_path, "PhysxVehicleContextAPI")
                })
            } else {
                false
            };
            if is_structural_binding(&view, &sink_sdf, sink_conn)
                && (!matches!(sink_conn, "drive" | "steer") || wheel_source_is_structural)
            {
                continue;
            }
            // Same reasoning, one level up — but for `outputs:` ONLY.
            //
            // An `outputs:` connection authored on a domain network root is read
            // at parse time by `domain_projection` and becomes an equation inside
            // the generated model (`soc = <battery>.soc_out;`). Its source prim is
            // a MEMBER of that island with no `SimComponent` of its own, so a
            // root output is consumed by the generated equation rather than a
            // second runtime wire. A direct cross-domain source such as
            // `</Rover/Battery.outputs:soc_out>` is different: the member-output
            // map below resolves it to the generated wrapper's declared output.
            // This keeps a public root boundary optional and never makes it the
            // apparent owner of the battery value.
            //
            // ⚠ NOT `inputs:`. A network root's `inputs:` are the island's
            // BOUNDARY — `read_network` declares them `input Real` on the
            // generated model and something OUTSIDE must drive them, which is
            // exactly a runtime wire. `rocker_bogie.usda` authors
            // the rover-root `inputs:drive_left.connect = </RockerBogie.outputs:drive_left>`;
            // skipping that would leave the island's demand inputs permanently
            // unwritten and every motor's electrical draw at zero.
            if attr.starts_with("outputs:")
                && crate::domain_projection::is_domain_network_root(&view, &sink_sdf)
                && lunco_usd_bevy::program::is_network_boundary_output(&view, &sink_sdf, &attr)
            {
                continue;
            }
            // A domain root's `inputs:` are live boundary wires, but no wire may
            // exist until projection has installed the generated Modelica model.
            // Before then there is no target contract to resolve against; creating
            // a SimConnection early only produces a false unknown-port warning on
            // the first fixed tick. `Added<ModelicaModel>` above re-runs this pass
            // when the contract arrives.
            if attr.starts_with("inputs:")
                && crate::domain_projection::is_domain_network_root(&view, &sink_sdf)
                && lunco_usd_bevy::program::internal_network_input_source(
                    &view, &sink_sdf, sink_conn,
                )
                .is_some()
            {
                continue;
            }
            if attr.starts_with("inputs:")
                && crate::domain_projection::is_domain_network_root(&view, &sink_sdf)
                && !has_modelica
            {
                continue;
            }
            // SSP `LinearTransformation`: the propagated value is `src * factor +
            // offset`. Authored on the sink prim, keyed by the consuming port
            // (`lunco:factor:<port>` / `:offset:<port>`), so each input carries its
            // own scaling. Absent ⇒ identity (1, 0), matching the pre-migration
            // `lunco:factor` default. The transform is invariant across the fan-in
            // sources, so it is read once per sink port, above the source loop.
            // Tolerant of `float` or `double` authoring — a wire naturally matches
            // the `float`-typed port it scales, so a strict `double` read would
            // silently drop the transform.
            let scale = view
                .real(&sink_sdf, &format!("lunco:factor:{sink_conn}"))
                .unwrap_or(1.0);
            let offset = view
                .real(&sink_sdf, &format!("lunco:offset:{sink_conn}"))
                .unwrap_or(0.0);

            // A PARAMETER IS AN INPUT WITH A CONSTANT INSTEAD OF A CONNECTION.
            // An `inputs:` port with no wire into it is authored data — `float
            // inputs:kv = 1.2` — and it is the ONLY way USD reaches a model's
            // parameters. Collected here (the one pass that already enumerates
            // every `inputs:` port with the composed reader in hand) and applied
            // by `seed_usd_input_defaults` once the model exists.
            let sources = view.connections(&sink_sdf, &attr);
            if sources.is_empty() {
                // An unconnected `outputs:` is just a declared port, not a parameter.
                if forward.is_some() {
                    continue;
                }
                if let Some(v) = view.real(&sink_sdf, &attr) {
                    defaults
                        .entry(entity)
                        .or_default()
                        .insert(sink_conn.to_string(), v);
                }
                continue;
            }

            for src in sources {
                // Split `/A.outputs:netForce` → prim `/A`, leaf `outputs:netForce`.
                let Some((src_prim, src_leaf)) = src.rsplit_once('.') else {
                    warn!(
                        "[usd-cosim] {}.{}: malformed connection source '{}' (no `.<connector>`)",
                        prim_path.path, attr, src
                    );
                    continue;
                };
                // No forward-following here. A forward is materialised as its own
                // edge at the prim that authors it (see `forward` above), so a
                // consumer just reads the port it named and a chain of forwards is a
                // chain of edges. Walking the chain from this side too would be a
                // second resolution path for the same fact.
                // The namespace says WHICH SIDE of the source to read. `outputs:` is
                // what it produces; `inputs:` is what it was commanded — a drive law
                // consumes the vessel's throttle command, and both can share a name
                // on one entity. Carried on the edge because propagation cannot
                // recover it later.
                let start_is_input = src_leaf.starts_with("inputs:");
                let src_conn = src_leaf
                    .strip_prefix("outputs:")
                    .or_else(|| src_leaf.strip_prefix("inputs:"))
                    .unwrap_or(src_leaf);

                // A composed member of a generated network has no standalone
                // `SimComponent`; its live outputs are public aliases on the
                // network wrapper. Resolve that address before looking for a
                // spawned member entity. This is the generic generated-network
                // boundary, not a special case for a propulsion or visual type.
                let generated_alias = (!start_is_input)
                    .then(|| {
                        generated_member_outputs.get(&(
                            prim_path.stage_handle.id(),
                            sink_instance,
                            src_prim.to_string(),
                            src_conn.to_string(),
                        ))
                    })
                    .flatten()
                    .cloned();
                let generated_alias_present = generated_alias.is_some();
                let (mut start_element, mut src_conn) = if let Some((wrapper, alias)) =
                    generated_alias
                {
                    (wrapper, alias)
                } else {
                    // A source path is absolute in the composed USD stage. An
                    // instance-local source must resolve in the sink's instance,
                    // but a scene-authored source (for example a kinematic landing
                    // target) intentionally lives outside that instance. Resolve
                    // the local namespace first, then the authored scene namespace.
                    // This keeps duplicated assets isolated without making a
                    // scene-level connection depend on which asset consumes it.
                    let source_key = (
                        prim_path.stage_handle.id(),
                        sink_instance,
                        src_prim.to_string(),
                    );
                    let scene_source_key =
                        (prim_path.stage_handle.id(), None, src_prim.to_string());
                    let source_entity = by_path.get(&source_key).copied().or_else(|| {
                        sink_instance
                            .is_some()
                            .then(|| by_path.get(&scene_source_key).copied())
                            .flatten()
                    });
                    let Some(element) = source_entity else {
                        // Two very different situations, and they must not look alike.
                        // A prim that EXISTS on the stage but has no entity yet is
                        // mid-spawn: its later spawn is a structural change that re-runs
                        // this and completes the edge. A prim that is not on the stage at
                        // all is a typo'd or stale target that will never resolve, and a
                        // silently dropped wire is how a vehicle ends up with no forces
                        // and no explanation.
                        if let Ok(src_sdf) = SdfPath::new(src_prim) {
                            if !view.has_prim(&src_sdf) {
                                warn!(
                                    "[usd-cosim] {}.{}: connection source '{}' names a prim that does \
                                     not exist on this stage — the wire is dropped. Check the path.",
                                    prim_path.path, attr, src_prim
                                );
                            }
                        }
                        continue;
                    };
                    (element, src_conn.to_string())
                };
                if !start_is_input
                    && environment_probe_entities.contains(&start_element)
                    && matches!(
                        src_conn.as_str(),
                        lunco_cosim::EARTH_MOUNT_X_CONNECTOR
                            | lunco_cosim::EARTH_MOUNT_Y_CONNECTOR
                            | lunco_cosim::EARTH_MOUNT_Z_CONNECTOR
                    )
                {
                    earth_direction_required.insert(start_element);
                }

                // ── The SOURCE side of the runtime-output indirection ────────
                // A vessel's `outputs:drive_left` is not stored on the vessel
                // prim: `OutputPorts` realises it as a child `Port` entity, and
                // that is where `apply_drive_mix` writes. The sink side above has
                // always redirected onto that child; reading one had no such hop,
                // so a wire whose SOURCE is a vessel actuator port resolved to the
                // vessel entity, found no port of that name, and delivered its
                // default forever.
                //
                // MEASURED on `scenes/tests/solar_domain_nested_ref.usda`: the
                // rover's `throttle` reached 1.0 and the skid kernel wrote both
                // bank ports, while the rover-root drive input — wired from
                // `</RockerBogie.outputs:drive_left>` — stayed at 0.0 for the whole
                // run. Every motor drew no current, so a driving rover's battery
                // never discharged and its bus was solved as if parked. Silent:
                // the island compiled, published, and stepped.
                if !generated_alias_present && !start_is_input {
                    if let Some(port_entity) = port_surfaces
                        .get(&start_element)
                        .and_then(|surface| surface.get(&src_conn))
                        .or_else(|| {
                            q_outputs
                                .get(start_element)
                                .ok()
                                .and_then(|outputs| outputs.get(&src_conn))
                        })
                    {
                        start_element = port_entity;
                        src_conn = lunco_cosim::PORT_NAME.to_string();
                    }
                }

                // ── The realtime gate ───────────────────────────────────────
                // A program may only push a client-predicted `Dynamic` body around
                // if it PROMISED it steps fast enough
                // (`lunco:program:realtimeSafe = true`). Without that promise — the
                // common case, since the default is `false` — an adaptive,
                // variable-cost solver is deciding the forces inside the prediction
                // loop, and the body diverges from the server every frame the solver
                // runs late.
                //
                if client_predicts
                    && lunco_cosim::is_physics_force_port(sink_conn)
                    && matches!(
                        q_predicted_body.get(entity),
                        Ok(avian3d::prelude::RigidBody::Dynamic)
                    )
                    && q_realtime_safe.get(start_element).is_err()
                {
                    let source_prim = src_prim.to_string();
                    let detail = format!(
                        "{}.{} drives predicted dynamic body {} without \
                         `lunco:program:realtimeSafe = true`; the force wire was not admitted",
                        prim_path.path, attr, source_prim,
                    );
                    error!("[usd-cosim] {detail}");
                    commands.queue(move |world: &mut World| {
                        let raised = world
                            .get_resource_mut::<lunco_core::RuntimeFaults>()
                            .is_some_and(|mut faults| {
                                faults.raise(
                                    "cosim-predicted-force-contract",
                                    Some(entity),
                                    source_prim,
                                    detail,
                                )
                            });
                        if raised {
                            if let Some(mut holds) =
                                world.get_resource_mut::<lunco_physics::PhysicsHolds>()
                            {
                                holds.set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
                            }
                        }
                    });
                    continue;
                }

                let end = if let Some(surface) = wheel_endpoints {
                    surface
                        .get(sink_conn)
                        .map(|port| (port, lunco_cosim::PORT_NAME.to_string()))
                } else {
                    Some(
                        forward
                            .clone()
                            .unwrap_or_else(|| (entity, sink_conn.to_string())),
                    )
                };
                let Some((end_element, end_connector)) = end else {
                    debug!(
                        "[usd-cosim] deferring wheel {}.{} until its authored physical endpoint exists",
                        prim_path.path, sink_conn
                    );
                    continue;
                };
                commands.spawn((
                    SimConnection {
                        start_element,
                        start_connector: src_conn.to_string(),
                        start_is_input,
                        end_element,
                        end_connector,
                        scale,
                        offset,
                    },
                    UsdWiredConnection,
                    // Keep the immutable USD fact on the derived runtime edge.
                    // The generic binder has no USD dependency, but its terminal
                    // diagnostics still need to name the authored source and
                    // sink that must be repaired.
                    Name::new(format!("UsdWire {src} -> {}.{sink_conn}", prim_path.path)),
                    // A derived edge is a PURE CACHE of USD wiring — every peer
                    // re-derives it from the same stage, so it must never carry
                    // network identity. `Local` is not a micro-optimisation here,
                    // it closes a FEEDBACK LOOP: untagged entities fall into the
                    // `None` arm of `assign_global_entity_ids` (lunco-core) and
                    // get an auto-allocated id, which makes this system's own
                    // `Added<GlobalEntityId>` gate fire on the very next frame,
                    // which despawns and respawns every edge, which mints fresh
                    // ids… Steady state cost was a full wiring rebuild EVERY
                    // FRAME (8.6 ms on sandbox_scene) with nothing changing.
                    // See docs/architecture/42-ui-frame-discipline.md §6.
                    lunco_core::Provenance::Local,
                ));
            }
        }
    }

    // Publish the authored parameters — but ONLY where they changed. This runs on
    // every structural change (any prim spawning anywhere re-runs the whole pass),
    // and `seed_usd_input_defaults` reacts to `Changed`. Re-inserting an identical
    // map would fire `Changed` anyway and re-seed the model, clobbering a value a
    // script had since written through `SetPorts` — an autopilot's `engage` would
    // silently snap back to its authored default the next time anything spawned.
    for (entity, map) in defaults {
        if q_defaults.get(entity).map(|d| d.0 != map).unwrap_or(true) {
            commands.entity(entity).try_insert(UsdInputDefaults(map));
        }
    }

    for entity in earth_direction_required {
        commands
            .entity(entity)
            .try_insert(lunco_environment::EarthDirectionRequired);
    }
}

/// Whether the causal-participant projection needs to be recomputed.
///
/// Connections are an immutable derived cache, so additions/removals are the
/// normal topology revision. Endpoint lifecycle transitions are included as
/// well: a wire can become executable after a joint, wheel, or model surface
/// is admitted. BindingRevision covers the scene epoch seal, which is the
/// fail-closed boundary while projection is still incomplete.
fn causal_participants_changed(
    arrivals: Query<
        (),
        Or<(
            Added<SimConnection>,
            Changed<SimConnection>,
            Changed<ConnectionBinding>,
            Added<ModelicaModel>,
            Added<lunco_core::CausalStateSink>,
        )>,
    >,
    mut removed_connections: RemovedComponents<SimConnection>,
    mut removed_bindings: RemovedComponents<ConnectionBinding>,
    mut removed_models: RemovedComponents<ModelicaModel>,
    mut removed_sinks: RemovedComponents<lunco_core::CausalStateSink>,
    revision: Res<lunco_cosim::BindingRevision>,
) -> bool {
    !arrivals.is_empty()
        || removed_connections.read().next().is_some()
        || removed_bindings.read().next().is_some()
        || removed_models.read().next().is_some()
        || removed_sinks.read().next().is_some()
        || revision.is_changed()
}

/// Recompute the shared-clock participant set from the resolved causal graph.
///
/// A Modelica model is a shared-clock participant when, following authored
/// causal connections backwards, it can reach a stateful engine sink:
///
/// * an input on a backend-owned CausalStateSink endpoint.
///
/// The reverse closure also captures intermediate Modelica or script nodes, so
/// a model feeding another model that eventually drives a body remains coupled.
/// Telemetry, electrical, or supervisory outputs do not become barriers merely
/// because those models are live.
///
/// The projection is fail-closed until the binding epoch is sealed and every
/// connection has reached a terminal binding state. During that interval every
/// live Modelica participant is treated as coupled, so an incomplete graph
/// cannot accidentally release a causal participant.
fn derive_causal_barrier_participants(world: &mut World) {
    let modelica_entities: bevy::ecs::entity::EntityHashSet = world
        .query_filtered::<Entity, With<ModelicaModel>>()
        .iter(world)
        .collect();

    let stateful_sinks: bevy::ecs::entity::EntityHashSet = world
        .query_filtered::<Entity, With<lunco_core::CausalStateSink>>()
        .iter(world)
        .collect();

    let connections: Vec<(Entity, Entity, String, Option<ConnectionBinding>)> = world
        .query_filtered::<(&SimConnection, Option<&ConnectionBinding>), With<SimConnection>>()
        .iter(world)
        .map(|(connection, binding)| {
            (
                connection.start_element,
                connection.end_element,
                connection.end_connector.clone(),
                binding.cloned(),
            )
        })
        .collect();

    // Reverse adjacency: stateful sink <- upstream source. Only executable
    // edges participate in the causal closure. A failed edge is terminal for
    // binding/readiness, but it is not a live signal path and must not couple
    // an otherwise independent participant to the shared clock.
    let mut upstream: std::collections::HashMap<Entity, Vec<Entity>> =
        std::collections::HashMap::new();
    for (start, end, _, binding) in &connections {
        if matches!(binding, Some(ConnectionBinding::Bound)) {
            upstream.entry(*end).or_default().push(*start);
        }
    }

    let mut causal_entities = stateful_sinks.clone();
    let mut frontier: Vec<Entity> = stateful_sinks.into_iter().collect();
    while let Some(sink) = frontier.pop() {
        for source in upstream.get(&sink).into_iter().flatten().copied() {
            if causal_entities.insert(source) {
                frontier.push(source);
            }
        }
    }

    let participants: Vec<Entity> = modelica_entities
        .iter()
        .copied()
        .filter(|entity| causal_entities.contains(entity))
        .collect();

    let bindings_terminal = connections.iter().all(|(_, _, _, binding)| {
        matches!(
            binding,
            Some(ConnectionBinding::Bound) | Some(ConnectionBinding::Failed)
        )
    });
    let topology_ready = world
        .get_resource::<lunco_cosim::BindingRevision>()
        .is_some_and(|revision| revision.sealed)
        && bindings_terminal;

    let mut projection = world.resource_mut::<lunco_core::SimulationBarrierParticipants>();
    if topology_ready {
        projection.replace(participants);
    } else {
        projection.topology_ready = false;
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
                        prim_path.path,
                        port,
                        sim.model_name,
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

// ── Uniform port reads (ListPorts / GetPort) ────────────────────────────────
//
// The single API surface over the cosim **port table** (`lunco_cosim::ports`).
// Every exposed value — Modelica var, Avian force/state, joint angle, env
// signal — is read/written/listed here uniformly, regardless of which backend
// owns it. These are the canonical port verbs; they are not aliases of
// `CosimStatus` (which stays as richer per-entity cosim introspection).

/// Map a [`lunco_cosim::PortDirection`] to a stable wire string.
fn port_dir_str(d: lunco_cosim::PortDirection) -> &'static str {
    match d {
        lunco_cosim::PortDirection::In => "in",
        lunco_cosim::PortDirection::Out => "out",
        lunco_cosim::PortDirection::InOut => "inout",
    }
}

fn port_to_json(p: &lunco_core::ports::PortRef) -> serde_json::Value {
    serde_json::json!({
        "name": p.name,
        "direction": port_dir_str(p.direction),
        "value": p.value,
    })
}

/// Resolve the optional `api_id` / `entity` field of a params object to an ECS
/// `Entity` via the `ApiEntityRegistry`. Returns `None` when absent (the
/// caller lists all) or when the id doesn't resolve.
fn resolve_param_entity(world: &World, params: &serde_json::Value) -> Option<Entity> {
    let raw = params
        .get("api_id")
        .or_else(|| params.get("entity"))
        .and_then(|v| v.as_u64())?;
    let reg = world.get_resource::<lunco_api::ApiEntityRegistry>()?;
    reg.resolve(&lunco_core::GlobalEntityId::from_raw(raw))
}

/// `ListPorts` — enumerate exposed ports. With `{"api_id": N}`, lists that
/// entity's ports; without, lists every registered entity that has any port.
///
/// `curl … {"type":"ExecuteCommand","command":"ListPorts","params":{"api_id":12345}}`
pub struct ListPortsProvider;

impl lunco_api::ApiQueryProvider for ListPortsProvider {
    fn name(&self) -> &'static str {
        "ListPorts"
    }
    fn execute(&self, world: &World, params: &serde_json::Value) -> lunco_api::ApiResponse {
        let ports_reg = world.resource::<lunco_core::ports::PortRegistry>().clone();
        // Single-entity form.
        if let Some(e) = resolve_param_entity(world, params) {
            let ports: Vec<_> = ports_reg
                .entity_ports(world, e)
                .iter()
                .map(port_to_json)
                .collect();
            return lunco_api::ApiResponse::ok(serde_json::json!({ "ports": ports }));
        }
        // All-entities form: snapshot the registry list first (owned), then
        // read ports — avoids holding the resource borrow across `entity_ports`.
        let Some(reg) = world.get_resource::<lunco_api::ApiEntityRegistry>() else {
            return lunco_api::ApiResponse::ok(serde_json::json!({ "entities": [] }));
        };
        let entries = reg.entities();
        let mut rows = Vec::new();
        for (api_id, e) in entries {
            let ports = ports_reg.entity_ports(world, e);
            if ports.is_empty() {
                continue;
            }
            rows.push(serde_json::json!({
                "api_id": api_id.get(),
                "name": world.get::<Name>(e).map(|n| n.as_str().to_string()).unwrap_or_default(),
                "ports": ports.iter().map(port_to_json).collect::<Vec<_>>(),
            }));
        }
        lunco_api::ApiResponse::ok(serde_json::json!({ "entities": rows }))
    }
}

/// `GetPort` — read one port value.
///
/// `curl … {"type":"ExecuteCommand","command":"GetPort","params":{"api_id":N,"name":"yaw"}}`
pub struct GetPortProvider;

impl lunco_api::ApiQueryProvider for GetPortProvider {
    fn name(&self) -> &'static str {
        "GetPort"
    }
    fn execute(&self, world: &World, params: &serde_json::Value) -> lunco_api::ApiResponse {
        let Some(e) = resolve_param_entity(world, params) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::EntityNotFound,
                "GetPort requires a resolvable `api_id`",
            );
        };
        let Some(name) = params.get("name").and_then(|v| v.as_str()) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::DeserializationError,
                "GetPort requires a `name`",
            );
        };
        let ports_reg = world.resource::<lunco_core::ports::PortRegistry>().clone();
        match ports_reg.read_port(world, e, name) {
            Some(value) => {
                lunco_api::ApiResponse::ok(serde_json::json!({ "name": name, "value": value }))
            }
            None => lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::DeserializationError,
                format!("no port `{}` on entity", name),
            ),
        }
    }
}

/// API query provider: `curl … {"type":"ExecuteCommand","command":"CosimStatus","params":{}}`
/// returns one row per USD-driven cosim entity with position, model
/// state, and propagated cosim values. The response also includes the
/// authoritative synchronization projection so a rate/worker diagnosis can
/// distinguish a causal barrier from an unrelated model still running on its
/// worker. Lets you probe the running binary without polling logs.
pub struct CosimStatusProvider;

impl lunco_api::ApiQueryProvider for CosimStatusProvider {
    fn name(&self) -> &'static str {
        "CosimStatus"
    }
    fn execute(&self, world: &World, _params: &serde_json::Value) -> lunco_api::ApiResponse {
        let Some(mut q) = QueryState::<
            (
                &Name,
                &Transform,
                Option<&SimComponent>,
                Option<&ModelicaModel>,
                Option<&avian3d::prelude::LinearVelocity>,
            ),
            With<UsdSourcedCosim>,
        >::try_new(world) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "CosimStatus: ECS query is unavailable",
            );
        };

        let entities: Vec<serde_json::Value> = q
            .iter(world)
            .map(|(name, tf, comp, model, lv)| {
                // Full input/output maps so any cosim signal is readable
                // (the solar tracker's `yaw`/`tracking_error`, the balloon's
                // `buoyancy`, …) — not just a hardcoded set. This is the
                // general "read cosim world state" surface.
                let outputs = comp
                    .map(|c| {
                        c.outputs
                            .iter()
                            .map(|(k, v)| (k.clone(), serde_json::json!(v)))
                            .collect::<serde_json::Map<_, _>>()
                    })
                    .unwrap_or_default();
                let inputs = comp
                    .map(|c| {
                        c.inputs
                            .iter()
                            .map(|(k, v)| (k.clone(), serde_json::json!(v)))
                            .collect::<serde_json::Map<_, _>>()
                    })
                    .unwrap_or_default();
                serde_json::json!({
                    "name": name.as_str(),
                    "y": tf.translation.y,
                    "yaw": tf.rotation.to_euler(EulerRot::YXZ).0,
                    "vy": lv.map(|v| v.0.y).unwrap_or(0.0),
                    "has_simcomponent": comp.is_some(),
                    "model": comp.map(|c| c.model_name.clone()).unwrap_or_default(),
                    "status": comp.map(|c| match &c.status {
                        SimStatus::Idle => "Idle".to_string(),
                        SimStatus::Compiling => "Compiling".to_string(),
                        SimStatus::Running => "Running".to_string(),
                        SimStatus::Paused => "Paused".to_string(),
                        SimStatus::Error(reason) => format!("Error: {reason}"),
                    }).unwrap_or_else(|| "Unbound".to_string()),
                    "modelica_var_count": model.map(|m| m.variables.len()).unwrap_or(0),
                    "modelica_paused": model.map(|m| m.paused).unwrap_or(false),
                    "modelica_current_time": model.map(|m| m.current_time).unwrap_or(0.0),
                    "modelica_target_time": model.map(|m| m.target_time).unwrap_or(0.0),
                    "modelica_communication_period_secs": model.and_then(|m| {
                        m.communication_period_secs.is_finite().then_some(m.communication_period_secs)
                    }),
                    "modelica_schedule_error": model.and_then(|m| {
                        m.validated_communication_period_secs().err()
                    }),
                    "modelica_next_communication_time": model
                        .map(|m| m.next_communication_time)
                        .unwrap_or(0.0),
                    "modelica_is_stepping": model.is_some_and(|m| m.is_stepping),
                    // The Modelica worker's durable failure verdict is the
                    // reason readiness may be holding the world. Surface it
                    // here beside timing/ports so live API diagnosis does not
                    // require access to the process log.
                    "modelica_error": model.and_then(|m| m.last_error.clone()),
                    "outputs": outputs,
                    "inputs": inputs,
                })
            })
            .collect();
        let barrier = world
            .get_resource::<lunco_core::SimulationBarrier>()
            .copied()
            .unwrap_or_default();
        let (topology_ready, causal_participant_count) = world
            .get_resource::<lunco_core::SimulationBarrierParticipants>()
            .map(|participants| (participants.topology_ready, participants.entities.len()))
            .unwrap_or((false, 0));
        let causal_participants = world
            .get_resource::<lunco_core::SimulationBarrierParticipants>()
            .map(|participants| {
                participants
                    .entities
                    .iter()
                    .map(|entity| {
                        serde_json::json!({
                            "entity": entity.to_bits(),
                            "name": world.get::<Name>(*entity).map(Name::as_str),
                            "usd_path": world
                                .get::<UsdPrimPath>(*entity)
                                .map(|path| path.path.as_str()),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let Some(mut causal_sinks) =
            QueryState::<(), With<lunco_core::CausalStateSink>>::try_new(world)
        else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "CosimStatus: causal sink query is unavailable",
            );
        };
        let causal_sink_count = causal_sinks.iter(world).count();
        lunco_api::ApiResponse::ok(serde_json::json!({
            "entities": entities,
            "synchronization": {
                "barrier_held": barrier.held,
                "active_participants": barrier.active_participants,
                "shared_clock_participants": barrier.shared_clock_participants,
                "worst_lag_secs": barrier.worst_lag_secs,
                "worst_entity": barrier.worst_entity.map(Entity::to_bits),
                "topology_ready": topology_ready,
                "causal_participant_count": causal_participant_count,
                "causal_participants": causal_participants,
                "causal_sink_count": causal_sink_count,
            }
        }))
    }
}

/// API query provider for the native binding transaction. `CosimStatus` only
/// covers solver participants; this query exposes the other admission gates
/// that can legitimately keep the world ticket open (deferred USD stages,
/// joints, wheels, and differentials).
pub struct BindingStatusProvider;

impl lunco_api::ApiQueryProvider for BindingStatusProvider {
    fn name(&self) -> &'static str {
        "BindingStatus"
    }

    fn execute(&self, world: &World, _params: &serde_json::Value) -> lunco_api::ApiResponse {
        let Some(mut awaiting_query) =
            QueryState::<&UsdPrimPath, With<UsdAwaitingStage>>::try_new(world)
        else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: awaiting-stage query is unavailable",
            );
        };
        let awaiting = awaiting_query
            .iter(world)
            .map(|path| path.path.clone())
            .collect::<Vec<_>>();
        let Some(mut pending_joints_query) = QueryState::<
            (
                Entity,
                &UsdPrimPath,
                &lunco_usd_avian::PendingUsdJoint,
                Option<&lunco_core::Provenance>,
                Option<&lunco_core::GlobalEntityId>,
                Has<UsdInstanceRoot>,
            ),
            With<lunco_usd_avian::PendingUsdJoint>,
        >::try_new(world) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: pending-joint query is unavailable",
            );
        };
        let pending_joints = pending_joints_query
            .iter(world)
            .map(|(entity, path, joint, provenance, gid, is_instance_root)| {
                serde_json::json!({
                    "entity": entity.to_bits(),
                    "path": path.path,
                    "stage": format!("{:?}", path.stage_handle),
                    "joint_type": joint.joint_type,
                    "body0": joint.body0_path,
                    "body1": joint.body1_path,
                    "provenance": provenance.map(|value| format!("{value:?}")),
                    "gid": gid.map(|value| value.get()),
                    "instance_root": is_instance_root,
                })
            })
            .collect::<Vec<_>>();
        let Some(mut pending_wheels_query) =
            QueryState::<&UsdPrimPath, With<crate::PendingWheelWiring>>::try_new(world)
        else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: pending-wheel query is unavailable",
            );
        };
        let pending_wheels = pending_wheels_query
            .iter(world)
            .map(|path| path.path.clone())
            .collect::<Vec<_>>();
        let Some(mut pending_differentials_query) =
            QueryState::<&UsdPrimPath, With<crate::PendingDifferential>>::try_new(world)
        else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: pending-differential query is unavailable",
            );
        };
        let pending_differentials = pending_differentials_query
            .iter(world)
            .map(|path| path.path.clone())
            .collect::<Vec<_>>();

        let pending_body_paths = pending_joints
            .iter()
            .filter_map(|joint| {
                let object = joint.as_object()?;
                Some([
                    object.get("body0")?.as_str()?.to_string(),
                    object.get("body1")?.as_str()?.to_string(),
                ])
            })
            .flatten()
            .collect::<std::collections::BTreeSet<_>>();
        let Some(mut bodies_query) = QueryState::<(
            Entity,
            &UsdPrimPath,
            Option<&avian3d::prelude::RigidBody>,
            Option<&avian3d::prelude::Position>,
            Option<&avian3d::prelude::RigidBodyDisabled>,
            Option<&lunco_usd_avian::big_space_bridge::BridgeShadow>,
            Option<&lunco_core::Provenance>,
            Option<&lunco_core::GlobalEntityId>,
            Has<UsdInstanceRoot>,
        )>::try_new(world) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: body query is unavailable",
            );
        };
        let bodies = bodies_query
            .iter(world)
            .filter(|(_, path, _, _, _, _, _, _, _)| pending_body_paths.contains(&path.path))
            .map(
                |(
                    entity,
                    path,
                    body,
                    position,
                    disabled,
                    shadow,
                    provenance,
                    gid,
                    is_instance_root,
                )| {
                    serde_json::json!({
                        "entity": entity.to_bits(),
                        "path": path.path,
                        "stage": format!("{:?}", path.stage_handle),
                        "rigid_body": body.map(|body| format!("{body:?}")),
                        "has_position": position.is_some(),
                        "disabled": disabled.is_some(),
                        "shadow_seeded": shadow.map(|shadow| shadow.is_seeded()),
                        "provenance": provenance.map(|value| format!("{value:?}")),
                        "gid": gid.map(|value| value.get()),
                        "instance_root": is_instance_root,
                    })
                },
            )
            .collect::<Vec<_>>();

        let mut non_terminal_models = Vec::new();
        let Some(mut models) = QueryState::<
            (&Name, Option<&ModelicaModel>, Option<&SimComponent>),
            With<UsdSourcedCosim>,
        >::try_new(world) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: model query is unavailable",
            );
        };
        for (name, model, component) in models.iter(world) {
            if !modelica_models_terminal(std::iter::once((model, component))) {
                non_terminal_models.push(serde_json::json!({
                    "name": name.as_str(),
                    "has_model": model.is_some(),
                    "has_simcomponent": component.is_some(),
                    "status": component.map(|c| format!("{:?}", c.status)),
                }));
            }
        }

        let Some(mut connections_count_query) =
            QueryState::<(), With<SimConnection>>::try_new(world)
        else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: connection query is unavailable",
            );
        };
        let connection_count = connections_count_query.iter(world).count();
        // `connection_count` alone cannot distinguish a correctly derived wire
        // from a wire that is still pending or bound to the wrong endpoint. Keep
        // the complete, generic edge inventory behind the same read-only API so
        // callers can diagnose authored USD topology without log scraping or
        // campaign-specific probes.
        let Some(mut connection_specs_query) = QueryState::<
            (
                Entity,
                &SimConnection,
                Option<&lunco_cosim::ConnectionBinding>,
                Has<lunco_cosim::BoundConnection>,
            ),
            With<SimConnection>,
        >::try_new(world) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: connection detail query is unavailable",
            );
        };
        let connection_specs = connection_specs_query
            .iter(world)
            .map(|(edge, spec, binding, bound)| {
                let endpoint = |entity: Entity| {
                    serde_json::json!({
                        "entity": entity.to_bits(),
                        "name": world.get::<Name>(entity).map(|name| name.as_str()),
                        "usd_path": world
                            .get::<UsdPrimPath>(entity)
                            .map(|path| path.path.as_str()),
                    })
                };
                serde_json::json!({
                    "edge": edge.to_bits(),
                    "source": endpoint(spec.start_element),
                    "source_port": spec.start_connector,
                    "source_is_input": spec.start_is_input,
                    "sink": endpoint(spec.end_element),
                    "sink_port": spec.end_connector,
                    "scale": spec.scale,
                    "offset": spec.offset,
                    "binding": binding.map(|value| format!("{value:?}")),
                    "bound": bound,
                })
            })
            .collect::<Vec<_>>();
        let Some(mut pending_revolute_query) = QueryState::<
            (),
            With<lunco_usd_avian::PendingJoint<avian3d::prelude::RevoluteJoint>>,
        >::try_new(world) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: pending revolute-joint query is unavailable",
            );
        };
        let Some(mut pending_prismatic_query) = QueryState::<
            (),
            With<lunco_usd_avian::PendingJoint<avian3d::prelude::PrismaticJoint>>,
        >::try_new(world) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: pending prismatic-joint query is unavailable",
            );
        };
        let Some(mut pending_fixed_query) = QueryState::<
            (),
            With<lunco_usd_avian::PendingJoint<avian3d::prelude::FixedJoint>>,
        >::try_new(world) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: pending fixed-joint query is unavailable",
            );
        };
        let pending_avian_joints = serde_json::json!({
            "revolute": pending_revolute_query.iter(world).count(),
            "prismatic": pending_prismatic_query.iter(world).count(),
            "fixed": pending_fixed_query.iter(world).count(),
        });
        let Some(mut pending_admission_query) = QueryState::<
            (
                Entity,
                &lunco_usd_avian::PendingJointAdmission,
                Option<&UsdPrimPath>,
            ),
            With<lunco_usd_avian::PendingJointAdmission>,
        >::try_new(world) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: joint-admission query is unavailable",
            );
        };
        let pending_admission_details = pending_admission_query
            .iter(world)
            .map(|(joint_entity, pending, path)| {
                let body = |entity: Entity| {
                    let rb = world.get::<avian3d::prelude::RigidBody>(entity);
                    let body_path = world.get::<UsdPrimPath>(entity);
                    serde_json::json!({
                        "entity": entity.to_bits(),
                        "path": body_path.map(|value| value.path.clone()),
                        "rigid_body": rb.map(|value| format!("{value:?}")),
                        "has_solver_body": world
                            .get::<avian3d::dynamics::solver::solver_body::SolverBody>(entity)
                            .is_some(),
                        "has_island_node": world
                            .get::<avian3d::dynamics::solver::islands::BodyIslandNode>(entity)
                            .is_some(),
                        "disabled": world
                            .get::<avian3d::prelude::RigidBodyDisabled>(entity)
                            .is_some(),
                        "ecs_disabled": world
                            .get::<bevy::ecs::entity_disabling::Disabled>(entity)
                            .is_some(),
                    })
                };
                serde_json::json!({
                    "joint_entity": joint_entity.to_bits(),
                    "joint_path": path.map(|value| value.path.clone()),
                    "body0": body(pending.body0),
                    "body1": body(pending.body1),
                })
            })
            .collect::<Vec<_>>();
        let Some(mut admitted_revolute_query) =
            QueryState::<(), With<avian3d::prelude::RevoluteJoint>>::try_new(world)
        else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: admitted revolute-joint query is unavailable",
            );
        };
        let Some(mut admitted_prismatic_query) =
            QueryState::<(), With<avian3d::prelude::PrismaticJoint>>::try_new(world)
        else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: admitted prismatic-joint query is unavailable",
            );
        };
        let Some(mut admitted_fixed_query) =
            QueryState::<(), With<avian3d::prelude::FixedJoint>>::try_new(world)
        else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "BindingStatus: admitted fixed-joint query is unavailable",
            );
        };
        let admitted_avian_joints = serde_json::json!({
            "revolute": admitted_revolute_query.iter(world).count(),
            "prismatic": admitted_prismatic_query.iter(world).count(),
            "fixed": admitted_fixed_query.iter(world).count(),
        });
        let wait_open = world.get_resource::<BindingEpochWait>().is_some();
        let dirty = world
            .get_resource::<BindingEpochDirty>()
            .is_some_and(|dirty| dirty.0);
        let physics_paused = world
            .get_resource::<Time<avian3d::prelude::Physics>>()
            .is_some_and(|time| time.is_paused());
        let physics_holds = world
            .get_resource::<lunco_physics::PhysicsHolds>()
            .map(|holds| holds.reasons().collect::<Vec<_>>())
            .unwrap_or_default();
        let runtime_fault = world
            .get_resource::<lunco_core::RuntimeFaults>()
            .and_then(|faults| faults.first.as_ref())
            .map(|fault| {
                serde_json::json!({
                    "kind": fault.kind,
                    "entity": fault.entity.map(Entity::to_bits),
                    "subject": fault.subject,
                    "detail": fault.detail,
                })
            });

        lunco_api::ApiResponse::ok(serde_json::json!({
            "wait_open": wait_open,
            "dirty": dirty,
            "connection_count": connection_count,
            "connections": connection_specs,
            "pending_avian_joints": pending_avian_joints,
            "admitted_avian_joints": admitted_avian_joints,
            "physics_paused": physics_paused,
            "physics_holds": physics_holds,
            "runtime_fault": runtime_fault,
            "pending_admission_details": pending_admission_details,
            "awaiting": awaiting,
            "pending_joints": pending_joints,
            "bodies": bodies,
            "pending_wheels": pending_wheels,
            "pending_differentials": pending_differentials,
            "non_terminal_models": non_terminal_models,
        }))
    }
}

/// Read-only camera/avatar inventory for diagnosing scene lifecycle failures.
///
/// [`lunco_api::ApiEntityRegistry`] is intentionally keyed by stable USD identity,
/// so two accidental ECS projections of the same prim collapse to one row in
/// `ListEntities`. This query instead enumerates the live ECS candidates by their
/// transient entity id and reports the roles that can make a duplicate visible.
/// It intentionally includes every `SceneCamera` intent, whether or not the
/// render host has attached Bevy's `Camera` component; unrelated render-only
/// cameras are not viewport candidates and are excluded.
pub struct SceneCameraAuditProvider;

impl lunco_api::ApiQueryProvider for SceneCameraAuditProvider {
    fn name(&self) -> &'static str {
        "SceneCameraAudit"
    }

    fn execute(&self, world: &World, _params: &serde_json::Value) -> lunco_api::ApiResponse {
        let Some(mut query) = QueryState::<
            (
                Entity,
                Option<&Name>,
                Option<&UsdPrimPath>,
                Option<&bevy::camera::Camera>,
                Option<&bevy::camera::RenderTarget>,
                Has<SceneCamera>,
                Has<lunco_usd_bevy::camera_mount::MountedCamera>,
                Has<Avatar>,
                Has<LocalAvatar>,
            ),
            With<SceneCamera>,
        >::try_new(world) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::InternalError,
                "SceneCameraAudit: ECS query is unavailable",
            );
        };
        let mut candidates: Vec<_> = query
            .iter(world)
            .map(
                |(entity, name, prim, camera, target, scene_camera, mounted, avatar, local)| {
                    serde_json::json!({
                        "entity": entity.to_bits(),
                        "name": name.map(|n| n.as_str()).unwrap_or_default(),
                        "usd_path": prim.map(|p| p.path.as_str()),
                        "stage": prim.map(|p| format!("{:?}", p.stage_handle.id())),
                        "scene_camera": scene_camera,
                        "mounted_camera": mounted,
                        "avatar": avatar,
                        "local_avatar": local,
                        // Headless production runs intentionally omit Bevy's
                        // render `Camera` component, but the render-free
                        // `SceneCamera` intent is still the authoritative
                        // viewport candidate. Keep the audit useful in both
                        // worlds instead of making headless diagnostics report
                        // an empty candidate set.
                        "render_camera": camera.is_some(),
                        "camera_active": camera.is_some_and(|camera| camera.is_active),
                        "camera_output_mode": camera.map(|camera| match camera.output_mode {
                            bevy::camera::CameraOutputMode::Write { .. } => "write",
                            bevy::camera::CameraOutputMode::Skip => "skip",
                        }),
                        "render_target": target.map(|target| match target {
                            bevy::camera::RenderTarget::Window(_) => "window",
                            bevy::camera::RenderTarget::Image(_) => "image",
                            bevy::camera::RenderTarget::TextureView(_) => "texture_view",
                            bevy::camera::RenderTarget::None { .. } => "none",
                        }),
                        "physical_target_size": camera
                            .and_then(|camera| camera.physical_target_size())
                            .map(|size| [size.x, size.y]),
                        "physical_viewport_size": camera
                            .and_then(|camera| camera.physical_viewport_size())
                            .map(|size| [size.x, size.y]),
                    })
                },
            )
            .collect();
        candidates.sort_by_key(|row| row["entity"].as_u64());
        lunco_api::ApiResponse::ok(serde_json::json!({
            "count": candidates.len(),
            "candidates": candidates,
        }))
    }
}

/// Reload (or load) a USD scene at runtime via the API.
///
/// `curl … {"type":"ExecuteCommand","command":"LoadScene","params":{"path":"lunco://scenes/luncosim/sandbox_scene.usda"}}`
///
/// - `path`: root-qualified USD address (`lunco://…` or `twin://…`).
/// - `root_prim`: optional override for the SDF path of the prim to
///   spawn. Empty (default) reads the stage's `defaultPrim` metadata;
///   if absent, the scene load fails visibly; a whole-stage `/` mount is not a
///   valid scene root.
///
/// Despawns every existing entity carrying `UsdPrimPath` plus every
/// `SimConnection` (cosim wires are scene-derived in current code), then
/// reloads the asset from disk and spawns a fresh root entity. Existing
/// pipelines (`sync_usd_visuals`, `process_usd_cosim_prims`, the
/// avian/sim translators) take it from there. The canonical `WorldGrid`
/// is used as the parent — i.e. the `BigSpace` host stays put across
/// reloads. Invalid world-shell topology is reported rather than repaired
/// or resolved by entity order.
///
/// Cleans up worker-side state too: sends `ModelicaCommand::Despawn`
/// for every entity carrying a `ModelicaModel` (the Modelica worker
/// drops its `steppers` / `cached_models` / `sim_streams` entries). Scene-owned
/// Rhai documents are stopped and closed by the shared `SceneTeardown` owner;
/// independent API/editor documents remain open until their explicit close.
/// Without these ownership boundaries, repeated reloads accumulate stale
/// workers or make an unrelated interactive document disappear.
#[Command(default)]
pub struct LoadScene {
    /// Root-qualified USD address (`lunco://…` or `twin://…`). Filesystem paths
    /// are opened through `OpenFile`, not this scene-mount command.
    pub path: String,
    /// Optional override for the prim to spawn. Empty (default) reads
    /// `defaultPrim` from the stage's metadata header. A missing `defaultPrim`
    /// is a visible scene-load error; the runtime never mounts `/`.
    pub root_prim: String,
}

// The `LoadScene` OBSERVER lives in `lunco-usd`
// (`commands.rs::on_load_scene`), not here: mounting a scene has to resolve the
// requested path to its DOCUMENT first (a doc-backed scene must mount its
// composed `base ⊕ runtime`, never the base file), and the document registry
// lives one layer up. This crate owns the mount MECHANICS the observer drives —
// [`validate_scene_address`], [`resolve_root_prim`], [`clear_scene_entities`],
// [`spawn_scene_root_world`], [`SceneLoadInFlight`] — as its public mount API.

/// Reload the CURRENTLY-ACTIVE scene from disk — the "restart" verb.
///
/// [`LoadScene`] deliberately no-ops when asked to load the scene that is already
/// active (same path + root), so it cannot pick up on-disk edits to the LIVE
/// scene. `RestartScene` always clears the current scene's entities, force-reloads
/// its stage asset from disk (busting the asset cache), and respawns a single
/// fresh root — so editing a `.usda` then `restart_scene()` shows the change with
/// no duplicate instances. `reset_document` is interpreted by the document layer:
/// it is false for the normal preserve-edits restart and true only after the UI
/// has confirmed a full reset. The lifecycle mechanic still targets whichever
/// scene is loaded.
/// Paired with `pause()` this is the "reload-then-freeze" one-liner the workflow
/// wanted (`restart_scene(); pause();`).
#[Command(default)]
pub struct RestartScene {
    /// Discard the active file document's authored and runtime layers before
    /// remounting. Callers must obtain explicit user consent first.
    pub reset_document: bool,
}

#[on_command(RestartScene)]
fn on_restart_scene(
    trigger: On<RestartScene>,
    mut coordinator: ResMut<SceneTransitionCoordinator>,
) {
    match coordinator.admit(SceneTransitionRequest::restart(
        trigger.event().reset_document,
    )) {
        SceneTransitionAdmission::AlreadyActive => {
            info!("[restart-scene] restart is already in progress — no-op");
        }
        SceneTransitionAdmission::Queued => {
            info!("[restart-scene] queued behind the active scene transaction");
        }
        SceneTransitionAdmission::Admitted => {
            info!("[restart-scene] admitted for the next scene lifecycle phase");
        }
    }
}

fn execute_admitted_restart_scene(
    trigger: On<SceneTransitionAdmitted>,
    asset_server: Res<AssetServer>,
    mut coordinator: ResMut<SceneTransitionCoordinator>,
    mut commands: Commands,
    q_usd: Query<(Entity, &UsdPrimPath, Has<UsdSceneRoot>)>,
    scene: SceneEntities,
    mut mount_state: Option<ResMut<lunco_core::SceneMountState>>,
) {
    let SceneTransitionRequest::Restart { reset_document } = &trigger.event().request else {
        return;
    };

    // Every loaded prim shares the scene's stage handle. REUSE that handle (not a
    // freshly-resolved path) so the exact same asset — INCLUDING its source scheme
    // (`twin://…`, `lunco://…`) — is respawned. Resolving via `.path()` would
    // drop the scheme and load a *different* raw-file asset, breaking twin routing
    // (avatar/camera setup, composed runtime edits) and leaving a stale camera.
    let Some((_, upp, _)) = q_usd.iter().find(|(entity, _, is_scene_root)| {
        *is_scene_root
            && mount_state
                .as_deref()
                .is_none_or(|state| state.contains_root(*entity))
    }) else {
        warn!("[restart-scene] no scene is loaded — nothing to restart");
        coordinator.finish_noop();
        return;
    };
    let handle = upp.stage_handle.clone();
    // Full asset path WITH source scheme (owned, so `reload` doesn't need a
    // `'static` borrow), for the reload key + the scene-root label. `None` only
    // for a document-backed stage with no registered path — still respawnable
    // from the handle, just unlabelled.
    let asset_path = asset_server.get_path(handle.id()).map(|p| p.into_owned());
    let label = asset_path
        .as_ref()
        .map(|p| p.to_string())
        .unwrap_or_else(|| "restarted-scene".to_string());
    info!("[restart-scene] reloading `{}` from disk", label);

    // Reject late events and deferred projections from the outgoing root while
    // its recursive despawn is still queued.
    if let Some(state) = mount_state.as_deref_mut() {
        state.begin_replacement();
    }

    // Restart is an explicit replacement transaction. Cancel the old load
    // identity before reclaiming its parked entities.
    commands.remove_resource::<SceneLoadInFlight>();
    let transition = SceneTransition::Restart {
        path: label.clone(),
        root_prim: String::new(),
        reset_document: *reset_document,
    };
    coordinator.start(transition.clone());
    commands.insert_resource(SceneLoadInFlight {
        path: label.clone(),
        stage_id: handle.id(),
    });
    commands.trigger(lunco_core::SceneTransitionStarted { transition });
    let stage_id = handle.id();

    // Despawn the old scene + free worker-side state (shared with `ClearScene`).
    // Every scene-authored entity (incl. the Avatar camera) carries `UsdPrimPath`,
    // so `try_despawn` (hierarchy-recursive) tears the old camera down here — no
    // stale window camera survives into the fresh scene.
    clear_scene_entities(&mut commands, &scene);

    // Defer the asset reload itself as well as the respawn. Higher layers may
    // refresh a doc-backed Twin's composed overlay while handling this same
    // command; doing the read in this queued phase makes that refreshed source
    // the one this stage reload consumes, without making this sim-layer module
    // depend on the document registry.
    commands.queue(move |world: &mut World| {
        let reload_expected = asset_path.is_some();
        if let Some(ap) = asset_path {
            world.resource::<AssetServer>().reload(ap);
        }
        spawn_scene_root_with_stage(world, &label, "", handle);
        if !reload_expected {
            world.write_message(SceneStageAssetOutcome::Loaded { stage_id });
        }
    });
}

/// Clear the active scene — despawn every USD prim entity + cosim wire
/// and free the worker-side Modelica steppers / Python script docs they
/// referenced, leaving an empty viewport.
///
/// Fired when a Twin / folder opens with nothing to show — no
/// `[usd] default_scene`, or a plain folder with no USD content — so the
/// viewport reflects the newly opened folder instead of keeping the
/// previously loaded scene. (`LoadScene` does this same clear *before*
/// loading its new scene.) Also useful standalone over the API / MCP as
/// a "clear the world" verb.
#[Command(default)]
pub struct ClearScene {}

#[on_command(ClearScene)]
fn on_clear_scene(_trigger: On<ClearScene>, mut coordinator: ResMut<SceneTransitionCoordinator>) {
    match coordinator.admit(SceneTransitionRequest::clear()) {
        SceneTransitionAdmission::AlreadyActive => {
            info!("[clear-scene] clear is already in progress — no-op");
        }
        SceneTransitionAdmission::Queued => {
            info!("[clear-scene] queued behind the active scene transaction");
        }
        SceneTransitionAdmission::Admitted => {
            info!("[clear-scene] admitted for the next scene lifecycle phase");
        }
    }
}

fn execute_admitted_clear_scene(
    trigger: On<SceneTransitionAdmitted>,
    mut coordinator: ResMut<SceneTransitionCoordinator>,
    mut commands: Commands,
    scene: SceneEntities,
    mut mount_state: Option<ResMut<lunco_core::SceneMountState>>,
) {
    if trigger.event().request != SceneTransitionRequest::Clear {
        return;
    }

    info!("[clear-scene] clearing viewport");
    coordinator.start(SceneTransition::Clear);
    if let Some(state) = mount_state.as_deref_mut() {
        state.begin_replacement();
    }
    commands.trigger(lunco_core::SceneTransitionStarted {
        transition: SceneTransition::Clear,
    });
    // A clear invalidates a stage load that may still be waiting on an asset.
    // Without removing this identity, a late outcome from the outgoing stage
    // can close or assert against the replacement transaction.
    commands.remove_resource::<SceneLoadInFlight>();
    commands.remove_resource::<lunco_usd_bevy::FailedSceneLoad>();
    clear_scene_entities(&mut commands, &scene);
    commands.queue(|world: &mut World| {
        world.trigger(SceneTransitionCompleted {
            transition: SceneTransition::Clear,
        });
    });
}

/// Route dependency-light scene requests to the typed command that owns each
/// transition. Every caller, including tutorials, enters the same transaction
/// coordinator as the public command/API surface.
fn on_scene_transition_intent(trigger: On<SceneTransitionIntent>, mut commands: Commands) {
    match &trigger.event().request {
        SceneTransitionRequest::Load { path, root_prim } => {
            commands.trigger(LoadScene {
                path: path.clone(),
                root_prim: root_prim.clone(),
            });
        }
        SceneTransitionRequest::Clear => commands.trigger(ClearScene {}),
        SceneTransitionRequest::Restart { reset_document } => {
            commands.trigger(RestartScene {
                reset_document: *reset_document,
            });
        }
    }
}

fn dispatch_admitted_scene_transition(
    mut coordinator: ResMut<SceneTransitionCoordinator>,
    mut commands: Commands,
) {
    let Some(request) = coordinator.take_admitted() else {
        return;
    };
    commands.trigger(SceneTransitionAdmitted { request });
}

fn has_admitted_scene_transition(coordinator: Res<SceneTransitionCoordinator>) -> bool {
    coordinator.has_admitted()
}

fn on_scene_transition_completed(
    trigger: On<SceneTransitionCompleted>,
    mut coordinator: ResMut<SceneTransitionCoordinator>,
) {
    coordinator.finish(&trigger.event().transition);
}

fn on_scene_transition_failed(
    trigger: On<SceneTransitionFailed>,
    mut coordinator: ResMut<SceneTransitionCoordinator>,
) {
    coordinator.finish(&trigger.event().transition);
}

/// Despawn the current scene's USD entities, synthesized physics entities, and
/// cosim wires.
///
/// The shared SceneTeardown schedule runs first. Its subsystem owners stop
/// scenario runtimes, retire Avian graph membership, and reset scene-derived
/// resources while the outgoing entities still exist. The deferred despawns
/// below then reclaim every entity under the same ownership boundary.
///
/// Commands touching this query use fallible forms because several teardown
/// owners may have already reclaimed a target in the same transaction.
/// The scene-owned entities a teardown touches, bundled as one `SystemParam`.
///
/// Every scene-lifecycle observer — `LoadScene` (in `lunco-usd`), `ClearScene`,
/// `RestartScene` — needs exactly this set. Bundling keeps the mount API honest:
/// a caller drives a teardown without naming `WorldGrid`, `OriginAnchor` or the
/// cosim `SimConnection` wire type, so `lunco-usd` needs no dependency on
/// `lunco-cosim` to orchestrate a scene swap.
#[derive(bevy::ecs::system::SystemParam)]
pub struct SceneEntities<'w, 's> {
    grid: Query<'w, 's, (Entity, &'static Children), With<WorldGrid>>,
    origin: Query<'w, 's, Entity, With<OriginAnchor>>,
    /// Every active scene root identifies the USD stage whose generated prims
    /// belong to that scene.  A camera mount is allowed to move a prim directly
    /// under the persistent grid, so hierarchy alone is not sufficient to find
    /// all of these on teardown.
    scene_roots: Query<'w, 's, &'static UsdPrimPath, With<UsdSceneRoot>>,
    prims: Query<'w, 's, (Entity, &'static UsdPrimPath)>,
    parents: Query<'w, 's, &'static ChildOf>,
    wires: Query<'w, 's, Entity, With<SimConnection>>,
    /// Physics-created joint entities and world-anchor bodies have no USD prim
    /// path, so their explicit scene-ownership marker is the authoritative
    /// reclamation key.
    physics_owned: Query<'w, 's, Entity, With<lunco_usd_avian::ScenePhysicsOwned>>,
}

pub fn clear_scene_entities(commands: &mut Commands, scene: &SceneEntities) {
    // A scene is its entities AND the resources derived from it. Resources are
    // restored through the registry rather than named here, so a subsystem that
    // adds scene-derived state does not also have to edit this function — see
    // `lunco_core::SceneTeardown`.
    commands.queue(lunco_core::run_scene_teardown);

    let (q_grid, q_origin, q_scene_roots, q_prims, q_wires, q_physics_owned) = (
        &scene.grid,
        &scene.origin,
        &scene.scene_roots,
        &scene.prims,
        &scene.wires,
        &scene.physics_owned,
    );
    let mut despawned = 0usize;

    // Despawn all children of the WorldGrid (recursively), except the persistent OriginAnchor
    let grid_entity = q_grid.single().ok().map(|(entity, _)| entity);
    if let Ok((_, children)) = q_grid.single() {
        for child in children.iter() {
            if !q_origin.contains(child) {
                commands.entity(child).try_despawn();
                despawned += 1;
            }
        }
    }

    // Some scene-owned prims intentionally leave their authored hierarchy. In
    // particular, `resolve_camera_mounts` moves a mounted USD camera directly
    // beneath the grid so it can host the floating origin at full precision.
    // It therefore survives a root-only clear and keeps rendering alongside the
    // next scene's avatar.  Reclaim every prim from an active scene-root stage
    // as a second, stage-scoped ownership sweep. Preview stages are not
    // `UsdSceneRoot`s, so editor previews remain outside this lifecycle.
    let active_stage_ids: std::collections::HashSet<_> = q_scene_roots
        .iter()
        .map(|root| root.stage_handle.id())
        .collect();
    let mut stage_prim_despawns = 0usize;
    for (entity, prim) in q_prims.iter() {
        let already_covered_by_grid = grid_entity.is_some_and(|grid| {
            let mut current = entity;
            for _ in 0..1024 {
                if current == grid {
                    return true;
                }
                let Ok(parent) = scene.parents.get(current) else {
                    return false;
                };
                current = parent.parent();
            }
            false
        });
        if active_stage_ids.contains(&prim.stage_handle.id()) && !already_covered_by_grid {
            commands.entity(entity).try_despawn();
            stage_prim_despawns += 1;
        }
    }

    // Despawn any root-level derived connection wires (which are spawned as root entities)
    for e in q_wires.iter() {
        commands.entity(e).try_despawn();
        despawned += 1;
    }

    for e in q_physics_owned.iter() {
        commands.entity(e).try_despawn();
        despawned += 1;
    }
    info!(
        "[scene] cleanup: {despawned} grid children and {stage_prim_despawns} stage prims queued for despawn"
    );
    // Every scene clear resets the whole clock tree to defaults (doc 19 §11b): a sky
    // left detached at 100 000×, a scrubbed animation, a paused transport — none of it
    // may survive into the next scene. This is the single choke point all three reload
    // paths funnel through, so the reset lives here, not at each call site.
    commands.trigger(lunco_time::ResetTime {});
}

/// End scene-owned script documents before their USD entities are removed.
///
/// The generic driver normally notices a detached entity on its next fixed
/// tick. That is deliberately too late for a scene boundary: the old
/// `on_stop` hook could then publish commands into the replacement scene, and
/// the driver's compiled `this` state would remain alive across the swap. The
/// marker is the ownership declaration; interactive/API documents are not
/// touched here.
fn stop_scene_owned_scripts(world: &mut World) {
    let targets: Vec<(Entity, Option<u64>)> = {
        let mut query =
            world.query_filtered::<(Entity, Option<&ScriptedModel>), With<SceneOwnedScript>>();
        query
            .iter(world)
            .map(|(entity, model)| (entity, model.and_then(|m| m.document_id)))
            .collect()
    };

    for (entity, document_id) in targets {
        ScenarioDriver::<RhaiScenarioRuntime>::stop_entity(world, entity);
        if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
            entity_mut.remove::<ScriptedModel>();
        }
        if let Some(document_id) = document_id {
            if let Some(mut registry) = world.get_resource_mut::<ScriptRegistry>() {
                registry.documents.remove(&DocumentId::new(document_id));
            }
        }
    }
}

/// Despawn a single USD prim **subtree** (one runtime prim and its descendants).
///
/// Under Bevy 0.19's relationship system, despawning the root entity recursively
/// despawns all descendants. Component removal triggers (`On<Remove, T>`) fire automatically,
/// freeing any worker-side state (such as Modelica steppers or Python script documents)
/// via the reactive observers registered in `lunco-modelica` and `lunco-scripting`.
pub fn despawn_usd_subtree(world: &mut World, root: Entity) {
    if let Ok(em) = world.get_entity_mut(root) {
        em.despawn();
        info!("[scene] incremental despawn: entity {:?}", root);
    }
}

/// Spawn one new USD child prim into a live scene, mirroring the child branch of
/// [`lunco_usd_bevy::instantiate_usd_prim`] — the per-prim analogue of a full
/// scene-root mount, used by E2 incremental spawn ([`lunco_usd::live_consume`])
/// when a `Resync` reports a prim added to the composed document.
///
/// The caller resolves the live parent from the canonical stage identity and
/// passes that entity through. This keeps the constructor scoped to the
/// authoritative USD hierarchy instead of making a second world-wide path
/// lookup (which cannot distinguish separate mounts of the same stage).
/// It spawns the stub child for `path` under the already-resolved
/// `parent_entity`, with a pre-read transform `tf`, inheriting grid-anchoring +
/// instance membership from that parent. The `on_usd_prim_added` observer then
/// builds the subtree from the canonical stage.
///
/// The live-stage projection bridge resolves the parent and checks the changed
/// path before calling this function. Passing the entity is intentional: the
/// parent hierarchy is the identity boundary, so this low-level constructor
/// neither resolves a parent by path nor scans the world for an existing child.
/// The stage itself cannot be held across the spawn because it aliases the
/// world; the observer reads it afresh from `CanonicalStages`.
pub fn spawn_usd_child_under_parent(
    world: &mut World,
    parent_entity: Entity,
    path: &str,
    tf: Transform,
) -> Option<Entity> {
    let stage_handle = world
        .get::<UsdPrimPath>(parent_entity)?
        .stage_handle
        .clone();
    let parent_path = world.get::<UsdPrimPath>(parent_entity)?.path.clone();
    let (parent_prefix, _) = path.rsplit_once('/')?;
    let expected_parent_path = if parent_prefix.is_empty() {
        "/"
    } else {
        parent_prefix
    };
    if parent_path != expected_parent_path {
        warn!(
            "[usd-cosim] incremental spawn rejected for {path}: resolved parent is {parent_path}, expected {expected_parent_path}"
        );
        return None;
    }

    // Inherit grid-anchoring + instance membership from the parent exactly as
    // `instantiate_usd_prim` derives them for its children.
    let parent_member = world.get::<UsdInstanceMember>(parent_entity).cloned();
    let parent_is_root = world.get::<UsdInstanceRoot>(parent_entity).is_some();
    let member = parent_member.or_else(|| {
        parent_is_root.then(|| UsdInstanceMember {
            root: parent_entity,
            root_path: parent_path.to_string(),
        })
    });

    let base = (
        Name::new(path.to_string()),
        UsdPrimPath {
            stage_handle,
            path: path.to_string(),
        },
        tf,
        GlobalTransform::default(),
        Visibility::Visible,
        InheritedVisibility::VISIBLE,
        ViewVisibility::default(),
    );
    // A top-level child of the nested scene Grid carries its own CellCoord.
    // Deeper USD descendants remain plain children of their authored parent.
    let parent_is_grid = world.get::<Grid>(parent_entity).is_some();
    let entity = match member {
        Some(m) if parent_is_grid => world
            .spawn((base, ChildOf(parent_entity), m, CellCoord::default()))
            .id(),
        Some(m) => world.spawn((base, ChildOf(parent_entity), m)).id(),
        None if parent_is_grid => world
            .spawn((base, ChildOf(parent_entity), CellCoord::default()))
            .id(),
        None => world.spawn((base, ChildOf(parent_entity))).id(),
    };
    info!("[scene] incremental spawn: `{}` (entity {})", path, entity);
    Some(entity)
}

/// Validate a scene's address before it enters the shared scene lifecycle.
/// `LoadScene` accepts only the two registered, root-qualified asset schemes:
/// `lunco://` for the shipped library and `twin://` for an opened Twin.
/// Filesystem paths belong to `OpenFile` / startup root discovery and must not
/// be reinterpreted here.
pub fn validate_scene_address(path_in: &str) -> Option<String> {
    let valid_lunco = lunco_assets::parse_lunco_uri(path_in)
        .is_some_and(lunco_assets::asset_path::is_safe_relative_path);
    let valid_twin = lunco_assets::parse_twin_uri(path_in).is_some_and(|(name, rel)| {
        !name.is_empty() && lunco_assets::asset_path::is_safe_relative_path(rel)
    });
    if valid_lunco || valid_twin {
        return Some(path_in.to_string());
    }

    warn!(
        "[scene] `{path_in}` is not a root-qualified scene address — LoadScene takes \
         `lunco://…` or `twin://…`. Use OpenFile for a filesystem path."
    );
    None
}

/// Spawn a USD scene root directly under the canonical `WorldGrid` entity.
///
/// Shared by `LoadScene` (after its clear step) and `OpenFile` (additive
/// import). Blender-style no-op when the same `(asset, root_prim)` is
/// already mounted. Returns the spawned entity, or `None` on no-op /
/// missing or invalid `WorldGrid`.
pub fn spawn_scene_root_world(
    world: &mut World,
    path_in: &str,
    root_prim_in: &str,
) -> Option<Entity> {
    let asset_path = validate_scene_address(path_in)?;
    // File-backed source: the AssetServer reads + composes the on-disk
    // stage. lunco-usd's E1 projection takes the other door
    // ([`spawn_scene_root_with_stage`]) to mount a document's *composed*
    // (base ⊕ runtime) stage instead.
    let handle = world
        .resource::<AssetServer>()
        .load::<UsdStageAsset>(asset_path.clone());
    spawn_scene_root_with_stage(world, &asset_path, root_prim_in, handle)
}

/// The mounted scene root — the entity a scene's whole prim subtree hangs from.
///
/// It is the only entity that knows **both** halves of "where does a scene-level
/// edit go?": its [`UsdPrimPath::stage_handle`] resolves to the editable document
/// (via `lunco_usd::twin_projection::scene_document_for`), and its
/// [`UsdPrimPath::path`] is the *mounted root prim* — `/SandboxScene`, `/World`,
/// `/HdriTest`, whatever this scene's `defaultPrim` happens to be.
///
/// Before this marker existed, a command that wanted to author a new top-level
/// prim had to guess at both (count the document registry; hardcode `/World`) —
/// and a hardcoded `/World` authors under a parent that does not exist in a scene
/// rooted at `/SandboxScene`, so the prim composes into the layer and is then
/// never mounted. The scene root is the answer to both questions; ask it.
///
/// The preview viewport (`lunco_usd::ui::viewport`) mounts its own private root
/// the same way, so consumers that must act on the *running* scene should scope
/// their query rather than assume a single one exists.
/// Spawn a USD scene root from an **already-built** stage handle.
///
/// The handle-supplying sibling of [`spawn_scene_root_world`]: instead of
/// loading the stage from disk via the `AssetServer`, the caller hands in a
/// `Handle<UsdStageAsset>` it built itself. This is the seam E1 uses — lunco-usd
/// passes a handle holding a [`UsdDocument`](../../lunco_usd/document)'s
/// *composed* (`base ⊕ runtime`) stage, so the live world projects the editable
/// document (with its persisted runtime spawns/moves) rather than the raw file.
///
/// `label` names the root (`Scene:{label}`) and feeds `defaultPrim` resolution;
/// `root_prim_in` empty defers the mount path to the stage's `defaultPrim`
/// (see [`resolve_root_prim`]). Missing `defaultPrim` is a terminal scene
/// error. Blender-style no-op when the same
/// `(handle, root_prim)` is already mounted. Returns the spawned entity, or
/// `None` on no-op.
pub fn spawn_scene_root_with_stage(
    world: &mut World,
    label: &str,
    root_prim_in: &str,
    handle: Handle<UsdStageAsset>,
) -> Option<Entity> {
    let asset_path = label.to_string();
    let root_prim = resolve_root_prim(&asset_path, root_prim_in);
    let new_id = handle.id();

    {
        let mut q = world.query::<&UsdPrimPath>();
        if q.iter(world)
            .any(|upp| upp.stage_handle.id() == new_id && upp.path == root_prim)
        {
            info!(
                "[scene] `{}` @ `{}` already loaded — no-op",
                asset_path, root_prim
            );
            return None;
        }
    }

    // Mount under the canonical world grid. `ensure_world_root` is create-or-get:
    // it builds the persistent shell (root + WorldGrid + persistent OriginAnchor)
    // the first scene load and returns the same grid on every reload — so the root
    // is never duplicated and never absent. Replaces the old "first `Grid` found"
    // heuristic, which was ambiguous once celestial / preview grids also existed.
    let grid = lunco_core::ensure_world_root(world);
    // Scene mounting owns the physics-frame binding. The canonical WorldGrid
    // is the scene frame; WorldRoot is only the persistent BigSpace shell and
    // must never become an implicit Avian frame.
    world.insert_resource(lunco_core::ActivePhysicsFrame(grid));

    // The scene root is the frame for its top-level USD prims as well as the
    // scene identity. Making it a nested Grid lets each top-level physical or
    // visual prim carry its own CellCoord, so a vehicle can cross a cell while
    // preserving the authored USD parentage and the same identity path.
    // Descendants remain plain children of their prim and use the low-precision
    // propagation path below that high-precision prim.
    // Register the mount before inserting `UsdPrimPath`. Adding that component
    // synchronously triggers the USD projection observer, which queues child
    // entities behind the scene-ownership fence. If the path were part of this
    // initial bundle, the observer could run before this root entered
    // `SceneMountState` and every queued child would be rejected as stale.
    //
    // The spatial components still land atomically with the root itself:
    // `ChildOf(grid)` + `CellCoord` + `Transform` are the same contract as
    // `migrate_to_grid`, avoiding the observer race that mis-tagged rover
    // chassis as `RigidBody::Static`.
    let scene_grid = world
        .get::<Grid>(grid)
        .cloned()
        .expect("ensure_world_root returned an entity without its Grid");
    let primary = world
        .get_resource::<SceneLoadInFlight>()
        .is_some_and(|load| load.stage_id == new_id && load.path == asset_path);
    let root = world
        .spawn((
            Name::new(format!("Scene:{}", asset_path)),
            UsdSceneRoot,
            scene_grid,
            Transform::default(),
            GlobalTransform::default(),
            Visibility::Visible,
            InheritedVisibility::default(),
            ViewVisibility::default(),
            CellCoord::default(),
            lunco_core::GridAnchor,
            ChildOf(grid),
        ))
        .id();
    if let Some(mut state) = world.get_resource_mut::<lunco_core::SceneMountState>() {
        state.register_root(root, primary);
    }
    world.entity_mut(root).insert(UsdPrimPath {
        stage_handle: handle,
        path: root_prim.clone(),
    });
    info!(
        "[scene] spawned `{}` @ `{}` (entity {})",
        asset_path, root_prim, root
    );
    Some(root)
}

/// Resolve the SDF mount path for a scene load.
///
/// Priority:
/// 1. explicit `override_in` (non-empty caller-supplied path) wins.
/// 2. otherwise return the empty *deferred-resolution sentinel* — the
///    scene-root entity is spawned with an empty path, and
///    `lunco_usd_bevy::instantiate_usd_prim` resolves it from the
///    stage's `defaultPrim` metadata once the asset has parsed
///    (a missing `defaultPrim` is a terminal scene error).
///
/// The defaultPrim lookup is deliberately deferred rather than read
/// here: this runs synchronously at command time, before the stage
/// asset finishes loading. It is resolved from the parsed canonical `StageView`
/// at instantiate time instead — correct on both native and web, and
/// yielding the defaultPrim subtree rather than a whole-stage `/` mount.
///
/// Per USD spec, `defaultPrim` is only required for files that will be
/// *referenced* by other USD files (composition arcs need a target
/// prim). Opening a stage directly works fine without it.
pub fn resolve_root_prim(_asset_path: &str, override_in: &str) -> String {
    if !override_in.is_empty() {
        return override_in.to_string();
    }
    // Deferred sentinel — resolved against the parsed stage downstream.
    String::new()
}

/// Plugin install hook — registers translator systems, per-tick sync
/// systems, and the API query provider. Called from `UsdSimPlugin::build`.
///
/// Opaque-body guard (prediction-membership design in git history): stamp
/// [`lunco_core::NotPredictable`] on every cosim-driven physics body — one with a
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
            Without<lunco_core::NotPredictable>,
        ),
    >,
) {
    for e in q.iter() {
        commands.entity(e).try_insert(lunco_core::NotPredictable);
    }
}

/// Per-tick ordering inside `FixedUpdate` matches the cosim master
/// algorithm:
///   `ModelicaSet::HandleResponses (Update) → sync_*_outputs →
///    PropagateCosimSet::Propagate → ApplyForcesCosimSet::ApplyForces →
///    sync_*_inputs → ModelicaSet::SpawnRequests`.
pub(crate) fn install(app: &mut App) {
    use lunco_cosim::systems::{
        apply_forces::CosimSet as ApplyForcesCosimSet, propagate::CosimSet as PropagateCosimSet,
    };
    use lunco_modelica::ModelicaSet;

    // Script execution is part of the fixed co-simulation transaction. Its
    // input snapshot is taken after propagation/actuation, its output becomes
    // visible on the next tick, and the transaction completes before the
    // Modelica master dispatches the next communication point.
    app.configure_sets(
        FixedUpdate,
        lunco_scripting::ScriptingSet.before(ModelicaSet::SpawnRequests),
    );

    // Scene-owned scripting state must end at the same boundary as the USD
    // entities that gave it meaning. This runs before clear_scene_entities'
    // deferred despawns, so the outgoing hook still sees the outgoing world.
    app.add_systems(lunco_core::SceneTeardown, stop_scene_owned_scripts);

    // Ensure the source asset types this module's systems read/allocate are
    // registered. Idempotent — production registers these via the Modelica /
    // scripting plugins; doing it here lets minimal apps (headless tests using
    // `MinimalPlugins` without those plugins) run the cosim systems without
    // panicking on a missing `Assets<…>` resource.
    app.init_asset::<ModelicaSource>()
        .init_asset::<PythonSource>()
        // The USD simulation projection owns these derived registries and
        // writes them even in a headless host.  Production Modelica setup also
        // initializes them, but minimal USD/physics apps intentionally omit
        // that plugin; keeping the resources here makes the projection
        // plugin's system contract complete and idempotent.
        .init_resource::<lunco_modelica::state::ModelicaDocumentRegistry>()
        .init_resource::<lunco_modelica::state::GeneratedModelicaSources>()
        .init_resource::<lunco_cosim::BindingRevision>()
        .init_resource::<lunco_core::SimulationBarrierParticipants>()
        .init_resource::<lunco_scripting::ScriptRegistry>()
        .init_resource::<WiringDirty>()
        .init_resource::<BindingEpochDirty>()
        .init_resource::<BindingModelStatuses>()
        .init_resource::<PythonUnavailablePrograms>()
        .init_resource::<crate::domain_projection::MemberClasses>()
        .init_resource::<crate::domain_projection::ProjectionDirty>()
        .init_resource::<crate::domain_projection::SynthesizerRegistry>()
        .init_resource::<UsdTelemetryProjectionIndex>()
        .init_resource::<PendingSceneStageOutcome>()
        .init_resource::<SceneTransitionCoordinator>();
    app.add_observer(request_binding_epoch::<UsdPrimPath>)
        .add_observer(request_binding_epoch_on_remove::<UsdPrimPath>)
        // Link port names are derived from the classes of the other authored
        // LinkNodes. A node arriving after its wire must therefore reopen the
        // same binding transaction as any other projected endpoint.
        .add_observer(request_binding_epoch::<lunco_celestial::link::LinkNode>)
        .add_observer(request_binding_epoch_on_remove::<lunco_celestial::link::LinkNode>)
        .add_observer(request_binding_epoch::<ModelicaModel>)
        .add_observer(request_binding_epoch_on_remove::<ModelicaModel>)
        .add_observer(crate::domain_projection::on_remove_generated_source)
        .add_observer(request_binding_epoch::<SimComponent>)
        .add_observer(forget_binding_model_status)
        .add_observer(request_binding_epoch::<lunco_usd_avian::PendingUsdJoint>)
        .add_observer(request_binding_epoch_on_remove::<lunco_usd_avian::PendingUsdJoint>)
        .add_observer(request_binding_epoch::<crate::PendingWheelWiring>)
        .add_observer(request_binding_epoch_on_remove::<crate::PendingWheelWiring>)
        .add_observer(request_binding_epoch::<crate::PendingDifferential>)
        .add_observer(request_binding_epoch_on_remove::<crate::PendingDifferential>)
        .add_observer(request_binding_epoch::<SimConnection>)
        .add_observer(request_binding_epoch_on_remove::<SimConnection>)
        .add_observer(on_scene_transition_intent)
        .add_observer(execute_admitted_restart_scene)
        .add_observer(execute_admitted_clear_scene)
        .add_observer(on_scene_transition_completed)
        .add_observer(on_scene_transition_failed);
    // USD source-load and contract failures use the same core notice stream as
    // the Modelica compiler, so the workbench console has one observable error
    // surface. `add_message` is idempotent when the Modelica plugin registered
    // it already.
    app.add_message::<lunco_modelica::ModelicaNotice>();
    app.add_message::<SceneStageAssetOutcome>();

    // A scene that is still spawning, and an object whose model has not
    // compiled, are the two things this module knows are not ready. Declaring
    // them is part of driving them — see `crate::readiness`.
    app.add_plugins(crate::readiness::UsdReadinessPlugin);

    app.configure_sets(
        Update,
        (
            CosimUpdateSet::Scene,
            CosimUpdateSet::Projection,
            CosimUpdateSet::Wiring,
        )
            .chain()
            .after(lunco_usd_bevy::process_queued_usd_visuals),
    );

    app.add_systems(
        Update,
        (
            publish_loaded_scene_stage_outcomes
                .after(lunco_usd_bevy::sync_usd_visuals)
                .run_if(on_message::<AssetEvent<UsdStageAsset>>),
            publish_failed_scene_stage_outcomes
                .run_if(on_message::<bevy::asset::AssetLoadFailedEvent<UsdStageAsset>>),
        ),
    );

    app.add_systems(
        First,
        (
            // `chain` inserts the synchronization point that makes the admitted
            // request visible here. Its trailing schedule flush applies the
            // replacement teardown before PreUpdate/Update consumers can query
            // the outgoing scene.
            dispatch_admitted_scene_transition.run_if(has_admitted_scene_transition),
            // Admitted-transition observers enqueue teardown and mount work;
            // make that whole queue land at this lifecycle boundary instead of
            // relying on Main's final implicit flush.
            ApplyDeferred,
        )
            .chain(),
    );

    app.add_systems(
        Last,
        // The terminal outcome is consumed at the end of the projection frame.
        // By `Last`, all USD, simulation, camera, render-binding, transform and
        // physics projection systems have run. A loaded outcome is retained by
        // `PendingSceneStageOutcome` while the bounded visual queue drains, so
        // this tiny state check is the only multi-frame lifecycle bookkeeping;
        // no heavyweight readiness scan runs on the UI thread.
        record_scene_load_terminal_outcome,
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
            .after(crate::UsdSimSet::ActivateDynamicBodies)
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
            // Python source-load drain runs every Update; cheap when no
            // `PendingPythonSource` entities exist. Splitting it from
            // `process_usd_cosim_prims` is intentional — the source asset may
            // take multiple frames to load (network on wasm, async I/O on
            // native).
            dispatch_loaded_python_sources,
            // Reads the class each member's `.mo` declares, so the projector
            // below instantiates what the file says rather than what its path
            // implies. Before it in the chain: a class landing this frame should
            // project this frame.
            crate::domain_projection::resolve_member_classes,
        )
            .chain()
            .in_set(CosimUpdateSet::Scene),
    );

    app.add_systems(
        Update,
        report_python_unavailable.after(CosimUpdateSet::Scene),
    );
    app.add_systems(lunco_core::SceneTeardown, reset_python_unavailable);
    app.add_systems(
        lunco_core::SceneTeardown,
        reset_usd_telemetry_projection_index,
    );

    app.add_systems(
        Update,
        crate::domain_projection::project_domain_islands
            .run_if(crate::domain_projection::domain_projection_due)
            .in_set(CosimUpdateSet::Projection),
    );
    app.add_systems(
        Update,
        mark_usd_telemetry_projection_index_dirty
            .after(crate::domain_projection::project_domain_islands)
            .run_if(telemetry_projection_index_changed)
            .in_set(CosimUpdateSet::Projection),
    );
    app.add_systems(
        Update,
        crate::domain_projection::sync_generated_network_documents
            .in_set(CosimUpdateSet::Projection),
    );
    app.add_systems(
        Update,
        crate::domain_projection::publish_generated_sources
            .after(crate::domain_projection::sync_generated_network_documents)
            .run_if(crate::domain_projection::generated_sources_need_publish)
            .in_set(CosimUpdateSet::Projection),
    );

    // Wiring is derived from native `connectionPaths`: rebuilds the
    // `SimConnection` set whenever prims spawn/despawn (structural) or a
    // `connectionPaths` edit is drained (`WiringDirty`); dormant otherwise.
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
            wrap_modelica_into_simcomponent.run_if(any_unwrapped_modelica),
            seed_usd_input_defaults,
            dispatch_loaded_modelica_sources,
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
            validate_usd_modelica_port_contracts.before(sync_modelica_outputs),
            // Scenario hooks read the public SimComponent surface directly.
            // Make the publication edge explicit: without this dependency a
            // fast cached compile could open the scenario gate and let Rhai
            // observe the wrapper before the first Modelica snapshot had been
            // copied into it. Cold runs happened to order the two systems the
            // other way, which made the first telemetry sample nondeterministic.
            sync_modelica_outputs
                .before(lunco_scripting::ScriptingSet)
                .before(PropagateCosimSet::Propagate),
            // Script backends consume the input snapshot and publish their
            // output snapshot as one fixed-step transaction. Script outputs
            // are published before the propagation phase, then the backend
            // executes after this tick's propagated inputs. That gives the
            // explicit one-tick causal delay required for a conservative
            // discrete co-simulation exchange and avoids an algebraic
            // same-tick script/physics cycle.
            sync_script_inputs
                .after(PropagateCosimSet::Propagate)
                .after(ApplyForcesCosimSet::ApplyForces)
                .before(lunco_scripting::ScriptingSet)
                .before(ModelicaSet::SpawnRequests),
            sync_script_outputs
                .before(lunco_scripting::ScriptingSet)
                .before(PropagateCosimSet::Propagate),
            sync_modelica_inputs
                .after(ApplyForcesCosimSet::ApplyForces)
                .before(ModelicaSet::SpawnRequests),
            // Modelica `when` bridge: edge-detect on fresh outputs, after they sync.
            fire_connected_events
                .after(sync_modelica_outputs)
                .after(sync_script_outputs)
                .before(PropagateCosimSet::Propagate),
        ),
    );

    app.add_systems(
        Startup,
        |reg: Option<ResMut<lunco_api::ApiQueryRegistry>>| {
            if let Some(mut reg) = reg {
                // Canonical uniform port reads (writes use the reflected
                // `lunco_cosim::SetPorts` command).
                reg.register(ListPortsProvider);
                reg.register(GetPortProvider);
                // Richer per-entity cosim introspection (not an alias of the above).
                reg.register(CosimStatusProvider);
                reg.register(BindingStatusProvider);
                // Lifecycle diagnostics must inspect raw ECS candidates: the normal
                // entity list is identity-deduplicated and cannot reveal two
                // projections of the same USD camera path.
                reg.register(SceneCameraAuditProvider);
                // The read path for `generated://…` models — the text a
                // projected USD network was actually compiled from.
                reg.register(crate::domain_projection::GeneratedSourceProvider);
            }
        },
    );

    // Registers the LoadScene type + observer (see register_commands! below).
    register_all_commands(app);
}

register_commands!(on_clear_scene, on_restart_scene,);

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct WiringRuns(usize);

    fn count_wiring_runs(mut runs: ResMut<WiringRuns>) {
        runs.0 += 1;
    }

    #[test]
    fn wiring_gate_is_dormant_until_a_real_trigger() {
        let mut app = App::new();
        app.init_resource::<WiringDirty>()
            .init_resource::<WiringRuns>()
            .add_systems(Update, count_wiring_runs.run_if(wiring_due));

        app.update();
        assert_eq!(app.world().resource::<WiringRuns>().0, 0);

        app.world_mut().spawn(UsdPrimPath::default());
        app.update();
        assert_eq!(app.world().resource::<WiringRuns>().0, 1);

        app.update();
        assert_eq!(app.world().resource::<WiringRuns>().0, 1);

        app.world_mut().resource_mut::<WiringDirty>().0 = true;
        app.update();
        assert_eq!(app.world().resource::<WiringRuns>().0, 2);

        app.world_mut().resource_mut::<WiringDirty>().0 = false;
        app.update();
        assert_eq!(app.world().resource::<WiringRuns>().0, 2);
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
            .add_systems(
                Update,
                count_telemetry_projection_runs.run_if(telemetry_projection_needed),
            );

        app.update();
        assert_eq!(app.world().resource::<TelemetryProjectionRuns>().0, 0);

        let entity = app.world_mut().spawn(UsdPrimPath::default()).id();
        app.update();
        assert_eq!(app.world().resource::<TelemetryProjectionRuns>().0, 1);

        app.world_mut()
            .entity_mut(entity)
            .insert(UsdTelemetryProjected);
        app.update();
        assert_eq!(app.world().resource::<TelemetryProjectionRuns>().0, 1);
    }

    #[test]
    fn telemetry_stage_revision_removes_derived_channels_and_markers() {
        let mut app = App::new();
        app.init_resource::<UsdTelemetryProjectionIndex>()
            .insert_resource(lunco_usd_bevy::UsdStageRevision(1))
            .add_systems(Update, mark_usd_telemetry_projection_index_dirty);
        let declaration = app.world_mut().spawn(UsdTelemetryProjected).id();
        let channel = app.world_mut().spawn(UsdTelemetryChannel).id();

        app.update();

        assert!(app
            .world()
            .get::<UsdTelemetryProjected>(declaration)
            .is_none());
        assert!(app.world().get_entity(channel).is_err());
        assert!(app.world().resource::<UsdTelemetryProjectionIndex>().dirty);
    }

    #[test]
    fn causal_barrier_is_the_reverse_closure_of_stateful_sinks() {
        let mut world = World::new();
        world.init_resource::<lunco_core::SimulationBarrierParticipants>();
        let mut revision = lunco_cosim::BindingRevision::default();
        revision.sealed = true;
        world.insert_resource(revision);

        let coupled = world.spawn(ModelicaModel::default()).id();
        let intermediate = world.spawn_empty().id();
        let telemetry_only = world.spawn(ModelicaModel::default()).id();
        let body = world
            .spawn((
                avian3d::prelude::RigidBody::Dynamic,
                lunco_core::CausalStateSink,
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

        let participants = world.resource::<lunco_core::SimulationBarrierParticipants>();
        assert!(participants.topology_ready);
        assert!(participants.entities.contains(&coupled));
        assert!(!participants.entities.contains(&telemetry_only));
        assert!(participants.requires_barrier(coupled));
        assert!(!participants.requires_barrier(telemetry_only));
    }

    #[test]
    fn unresolved_topology_keeps_the_barrier_fail_closed() {
        let mut world = World::new();
        world.init_resource::<lunco_core::SimulationBarrierParticipants>();
        let mut revision = lunco_cosim::BindingRevision::default();
        revision.sealed = true;
        world.insert_resource(revision);

        let model = world.spawn(ModelicaModel::default()).id();
        let body = world
            .spawn((
                avian3d::prelude::RigidBody::Dynamic,
                lunco_core::CausalStateSink,
            ))
            .id();
        world.spawn((SimConnection {
            start_element: model,
            end_element: body,
            end_connector: "force_y".into(),
            ..Default::default()
        },));

        derive_causal_barrier_participants(&mut world);

        let participants = world.resource::<lunco_core::SimulationBarrierParticipants>();
        assert!(!participants.topology_ready);
        assert!(participants.requires_barrier(model));
    }

    #[test]
    fn failed_edge_does_not_make_model_a_shared_clock_participant() {
        let mut world = World::new();
        world.init_resource::<lunco_core::SimulationBarrierParticipants>();
        let mut revision = lunco_cosim::BindingRevision::default();
        revision.sealed = true;
        world.insert_resource(revision);

        let model = world.spawn(ModelicaModel::default()).id();
        let body = world
            .spawn((
                avian3d::prelude::RigidBody::Dynamic,
                lunco_core::CausalStateSink,
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

        let participants = world.resource::<lunco_core::SimulationBarrierParticipants>();
        assert!(participants.topology_ready);
        assert!(!participants.entities.contains(&model));
        assert!(!participants.requires_barrier(model));
    }

    // ── resolve_root_prim ────────────────────────────────────────────
    //
    // `resolve_root_prim` no longer touches the filesystem: an explicit
    // override wins, and an empty override yields the deferred-resolution
    // sentinel (empty string). The actual `defaultPrim` lookup is done
    // from the parsed stage in `lunco_usd_bevy::instantiate_usd_prim`
    // (covered by `stage_default_prim` tests there) — correct on wasm too.

    #[test]
    fn resolve_root_prim_override_wins() {
        assert_eq!(resolve_root_prim("scene.usda", "/Override"), "/Override");
    }

    #[test]
    fn resolve_root_prim_empty_override_defers() {
        // Empty override → empty sentinel; resolved downstream against
        // the parsed stage, not here.
        assert_eq!(resolve_root_prim("scene.usda", ""), "");
    }

    #[test]
    fn scene_address_requires_a_registered_scheme_before_reload() {
        assert_eq!(
            validate_scene_address("lunco://scenes/luncosim/sandbox_scene.usda"),
            Some("lunco://scenes/luncosim/sandbox_scene.usda".to_string())
        );
        assert_eq!(
            validate_scene_address("twin://moonbase/scenes/sandbox_scene.usda"),
            Some("twin://moonbase/scenes/sandbox_scene.usda".to_string())
        );
        assert_eq!(
            validate_scene_address("scenes/luncosim/sandbox_scene.usda"),
            None
        );
        assert_eq!(
            validate_scene_address("/workspace/assets/scenes/luncosim/sandbox_scene.usda"),
            None
        );
        assert_eq!(validate_scene_address("lunco://"), None);
        assert_eq!(validate_scene_address("lunco://../scene.usda"), None);
        assert_eq!(validate_scene_address("twin:///scene.usda"), None);
    }

    // ── interface published at parse, not at solve ───────────────────
    //
    // The contract that killed the "dangling wire" false positive: a bound
    // model exposes its declared inputs BEFORE the worker has produced any
    // variables, so a wire into it resolves on the first propagation tick.

    #[test]
    fn usd_connection_properties_are_declared_as_scalar_ports() {
        assert_eq!(
            declared_port_name("inputs:earth_mount_x.connect", "inputs:"),
            Some("earth_mount_x".to_owned())
        );
        assert_eq!(
            declared_port_name("outputs:earth_mount_z", "outputs:"),
            Some("earth_mount_z".to_owned())
        );
        assert_eq!(declared_port_name("physics:mass", "inputs:"), None);
    }

    #[test]
    fn rigid_body_modelica_interface_leaves_physical_ports_to_avian() {
        let asset = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/vessels/landers/descent_lander.usda");
        let stage = lunco_usd_bevy::compose_file_to_stage(&asset).expect("compose lander asset");
        let view = lunco_usd_bevy::StageView::new(&stage);
        let root = SdfPath::new("/DescentLander").unwrap();
        let (mut inputs, _) = declared_interface(&view, &root);
        strip_rigid_body_inputs(&view, &root, &mut inputs);

        for physical in [
            "mass",
            "inertia_xx",
            "inertia_yy",
            "inertia_zz",
            "com_x",
            "com_y",
            "com_z",
        ] {
            assert!(
                !inputs.contains_key(physical),
                "{physical} is a rigid-body sink and must not shadow Avian"
            );
        }
        assert!(inputs.contains_key("controller_inertia_xx"));
    }

    #[test]
    fn environment_probe_publishes_schema_declared_output_contract() {
        let outputs = environment_probe_interface();
        assert_eq!(
            outputs
                .names
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            lunco_cosim::ENVIRONMENT_PROBE_OUTPUTS
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
        );
    }

    #[test]
    fn bare_acausal_interface_is_not_treated_as_an_unowned_wire() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/electrical_network.usda");
        let stage = lunco_usd_bevy::compose_file_to_stage(&path).expect("compose fixture");
        let view = lunco_usd_bevy::StageView::new(&stage);

        let bare_motor = SdfPath::new("/Rig/Motor").expect("motor path");
        let wired_battery = SdfPath::new("/Rig/Battery").expect("battery path");
        let bare_panel = SdfPath::new("/Rig/SolarPanel").expect("panel path");

        assert!(!has_connected_acausal_connector(&view, &bare_motor));
        assert!(has_connected_acausal_connector(&view, &wired_battery));
        assert!(!has_connected_acausal_connector(&view, &bare_panel));
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
        app.add_systems(Update, wrap_modelica_into_simcomponent);
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
        app.add_systems(Update, wrap_modelica_into_simcomponent);

        app.update();

        let declared = app
            .world()
            .get::<DeclaredOutputPorts>(entity)
            .expect("the generated wrapper must publish its complete output contract");
        assert!(declared.names.contains("soc"));
        assert!(declared
            .names
            .contains("__member_Rig_x2f_Battery_terminal_voltage_v"));
    }

    #[test]
    fn status_tracks_compile_run_pause_and_failure() {
        let mut model = dispatched_but_unsolved();
        assert_eq!(modelica_status(&model), SimStatus::Compiling);
        model.is_compiled = true;
        model.current_time = lunco_core::SECS_PER_TICK;
        assert_eq!(modelica_status(&model), SimStatus::Running);
        model.paused = true;
        assert_eq!(modelica_status(&model), SimStatus::Paused);
        model.last_error = Some("singular system".into());
        assert_eq!(
            modelica_status(&model),
            SimStatus::Error("singular system".into())
        );
    }

    #[derive(Resource, Default)]
    struct CapturedTelemetry(Vec<lunco_core::TelemetryEvent>);

    fn capture_telemetry(
        trigger: On<lunco_core::TelemetryEvent>,
        mut captured: ResMut<CapturedTelemetry>,
    ) {
        captured.0.push(trigger.event().clone());
    }

    #[test]
    fn connected_event_uses_authoritative_world_epoch() {
        let mut app = App::new();
        app.add_observer(capture_telemetry)
            .init_resource::<Time<Fixed>>()
            .insert_resource(lunco_time::WorldTime {
                epoch_jd: 2_451_600.25,
                ..default()
            })
            .init_resource::<CapturedTelemetry>();

        let mut source = SimComponent::default();
        source.outputs.insert("armed".into(), 1.0);
        app.world_mut().spawn((UsdPrimPath::default(), source));
        let event_path = UsdPrimPath {
            path: "/Event".into(),
            ..default()
        };
        app.world_mut().spawn((
            event_path,
            EventBinding {
                source_path: "/".into(),
                output: "armed".into(),
                name: "ARMED".into(),
                severity: lunco_core::Severity::Info,
                latched: false,
                qualification_time_s: 0.0,
                qualified_for_s: 0.0,
                armed: true,
            },
        ));
        app.add_systems(Update, fire_connected_events);
        app.update();

        let events = &app.world().resource::<CapturedTelemetry>().0;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].timestamp, 2_451_600.25);
    }

    #[test]
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

        copy_modelica_input_values(&mut model, &component, None);

        assert_eq!(model.inputs.get("guidance_throttle"), Some(&0.75));
        assert!(
            !model.inputs.contains_key("force_y"),
            "a physical sink must remain outside the Modelica input map"
        );
    }

    #[test]
    fn authored_command_surface_reaches_shared_modelica_input() {
        let mut model = dispatched_but_unsolved();
        model.inputs.insert("throttle".into(), 0.0);
        model.compiled_input_names.insert("throttle".into());
        let component = SimComponent::default();
        let command_surface = lunco_core::InputPorts::with_defaults([
            ("throttle".to_string(), 0.75),
            ("steer".to_string(), -0.2),
        ]);

        copy_modelica_input_values(&mut model, &component, Some(&command_surface));

        assert_eq!(model.inputs.get("throttle"), Some(&0.75));
        assert!(!model.inputs.contains_key("steer"));
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
            Some(lunco_core::Severity::Critical)
        );
    }

    #[derive(Resource, Default)]
    struct ProjectionFrameQueued(bool);

    #[derive(Resource)]
    struct ProjectionWritesApplied;

    #[derive(Resource, Default)]
    struct AdmittedRequests(Vec<SceneTransitionRequest>);

    #[derive(Resource, Default)]
    struct CompletedTransitions(Vec<SceneTransition>);

    #[test]
    fn explicit_stage_outcome_commits_without_readiness_polling() {
        let transition = SceneTransition::load("scene.usda", "/World");
        let stage_id = Handle::<UsdStageAsset>::default().id();
        let mut app = App::new();
        app.add_message::<SceneStageAssetOutcome>()
            .init_resource::<SceneTransitionCoordinator>()
            .init_resource::<PendingSceneStageOutcome>()
            .init_resource::<CompletedTransitions>()
            .add_observer(on_scene_transition_completed)
            .add_observer(
                |trigger: On<SceneTransitionCompleted>,
                 mut completed: ResMut<CompletedTransitions>| {
                    completed.0.push(trigger.event().transition.clone());
                },
            )
            .add_systems(
                Last,
                record_scene_load_terminal_outcome.run_if(on_message::<SceneStageAssetOutcome>),
            );

        {
            let mut coordinator = app.world_mut().resource_mut::<SceneTransitionCoordinator>();
            assert_eq!(
                coordinator.admit(SceneTransitionRequest::load("scene.usda", "/World")),
                SceneTransitionAdmission::Admitted
            );
            assert_eq!(
                coordinator.take_admitted(),
                Some(SceneTransitionRequest::load("scene.usda", "/World"))
            );
            coordinator.start(transition.clone());
        }
        app.insert_resource(SceneLoadInFlight {
            path: "scene.usda".to_owned(),
            stage_id,
        });
        app.world_mut()
            .write_message(SceneStageAssetOutcome::Loaded { stage_id });

        app.update();
        assert!(!app.world().contains_resource::<SceneLoadInFlight>());
        assert_eq!(
            app.world().resource::<CompletedTransitions>().0,
            vec![transition]
        );
        assert!(app
            .world()
            .resource::<SceneTransitionCoordinator>()
            .active()
            .is_none());
    }

    #[test]
    fn loaded_stage_outcome_waits_for_bounded_visual_projection() {
        let transition = SceneTransition::load("scene.usda", "/World");
        let stage_id = Handle::<UsdStageAsset>::default().id();
        let mut app = App::new();
        app.add_message::<SceneStageAssetOutcome>()
            .init_resource::<SceneTransitionCoordinator>()
            .init_resource::<PendingSceneStageOutcome>()
            .init_resource::<CompletedTransitions>()
            .add_observer(on_scene_transition_completed)
            .add_observer(
                |trigger: On<SceneTransitionCompleted>,
                 mut completed: ResMut<CompletedTransitions>| {
                    completed.0.push(trigger.event().transition.clone());
                },
            )
            .add_systems(Last, record_scene_load_terminal_outcome);

        {
            let mut coordinator = app.world_mut().resource_mut::<SceneTransitionCoordinator>();
            assert_eq!(
                coordinator.admit(SceneTransitionRequest::load("scene.usda", "/World")),
                SceneTransitionAdmission::Admitted
            );
            coordinator
                .take_admitted()
                .expect("the first scene request is admitted");
            coordinator.start(transition.clone());
        }
        let awaiting = app
            .world_mut()
            .spawn((
                UsdPrimPath {
                    stage_handle: Handle::default(),
                    path: "/World/HeavyMesh".to_owned(),
                },
                UsdAwaitingStage,
            ))
            .id();
        app.insert_resource(SceneLoadInFlight {
            path: "scene.usda".to_owned(),
            stage_id,
        });
        app.world_mut()
            .write_message(SceneStageAssetOutcome::Loaded { stage_id });

        app.update();
        assert!(app.world().contains_resource::<SceneLoadInFlight>());
        assert!(app.world().resource::<CompletedTransitions>().0.is_empty());

        app.world_mut()
            .entity_mut(awaiting)
            .remove::<UsdAwaitingStage>();
        app.update();
        assert!(!app.world().contains_resource::<SceneLoadInFlight>());
        assert_eq!(
            app.world().resource::<CompletedTransitions>().0,
            vec![transition]
        );
    }

    #[test]
    fn queued_transition_starts_after_the_projection_frame_flushes() {
        let first = SceneTransition::load("first.usda", "/World");
        let first_for_completion = first.clone();
        let second = SceneTransitionRequest::load("second.usda", "/World");
        let mut app = App::new();
        app.init_resource::<SceneTransitionCoordinator>()
            .init_resource::<ProjectionFrameQueued>()
            .init_resource::<AdmittedRequests>()
            .add_observer(on_scene_transition_completed)
            .add_observer(
                |trigger: On<SceneTransitionAdmitted>,
                 _flushed: Res<ProjectionWritesApplied>,
                 mut admitted: ResMut<AdmittedRequests>| {
                    admitted.0.push(trigger.event().request.clone());
                },
            )
            .add_systems(
                First,
                (
                    dispatch_admitted_scene_transition.run_if(has_admitted_scene_transition),
                    ApplyDeferred,
                )
                    .chain(),
            )
            .add_systems(
                Update,
                |queued: Res<ProjectionFrameQueued>, mut commands: Commands| {
                    if !queued.0 {
                        commands.insert_resource(ProjectionWritesApplied);
                    }
                },
            )
            .add_systems(
                Last,
                move |mut queued: ResMut<ProjectionFrameQueued>, mut commands: Commands| {
                    if !queued.0 {
                        queued.0 = true;
                        commands.trigger(SceneTransitionCompleted {
                            transition: first_for_completion.clone(),
                        });
                    }
                },
            );

        {
            let mut coordinator = app.world_mut().resource_mut::<SceneTransitionCoordinator>();
            assert_eq!(
                coordinator.admit(SceneTransitionRequest::load("first.usda", "/World")),
                SceneTransitionAdmission::Admitted
            );
            assert_eq!(
                coordinator.take_admitted(),
                Some(SceneTransitionRequest::load("first.usda", "/World"))
            );
            coordinator.start(first);
            assert_eq!(
                coordinator.admit(second.clone()),
                SceneTransitionAdmission::Queued
            );
        }

        app.update();
        assert!(app.world().resource::<AdmittedRequests>().0.is_empty());
        app.update();
        assert_eq!(app.world().resource::<AdmittedRequests>().0, vec![second]);
    }
}
