//! USD → cosim translator.
//!
//! Reads `lunco:modelicaModel` / `lunco:pythonModel` and native
//! `connectionPaths` from USD prims after `sync_usd_visuals` has spawned
//! the entity, and drives the full cosim lifecycle end-to-end:
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
//! No domain-specific markers (`BalloonModelMarker`, …) are inserted
//! here. The editor's catalog-driven authoring path owns its explicit
//! spawn markers; this translator is the authoritative path for
//! USD-defined cosim entities.

use avian3d::prelude::PhysicsTime;
use bevy::prelude::*;
use big_space::prelude::CellCoord;
use lunco_core::telemetry::{ChannelSource, Parameter};
use lunco_core::{
    on_command, register_commands, Avatar, Command, LocalAvatar, OriginAnchor, WorldGrid,
};
use lunco_cosim::{
    CosimOutputDescriptor, CosimOutputMetadata, DeclaredOutputPorts, SimComponent, SimConnection,
    SimStatus,
};
use lunco_doc::{DocumentId, DocumentOrigin};
use lunco_modelica::source_asset::ModelicaSource;
use lunco_modelica::{parse_model_interface, ModelicaChannels, ModelicaCommand, ModelicaModel};
use lunco_render::SceneCamera;
use lunco_scripting::source_asset::PythonSource;
use lunco_scripting::{
    doc::{ScriptDocument, ScriptLanguage, ScriptedModel},
    ScriptRegistry,
};
use lunco_usd_bevy::{
    CanonicalStages, UsdAwaitingStage, UsdInstanceMember, UsdInstanceRoot, UsdPrimPath, UsdRead,
    UsdStageAsset,
};
use openusd::sdf::Path as SdfPath;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

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

/// Telemetry event published when a USD-declared model could not be handed to
/// the solver at all — the worker channel was closed, so the compile that
/// `SimStatus::Compiling` is waiting for will never be attempted.
///
/// Published at [`lunco_core::Severity::Error`] so the workbench status bar's
/// error-telemetry observer surfaces it, the same arrangement
/// [`lunco_usd_bevy::SCENE_LOAD_FAILED`] uses. A scene whose models silently
/// never step is indistinguishable from a scene that is merely still compiling;
/// the difference has to reach the UI, not just the log.
pub const MODEL_DISPATCH_FAILED: &str = "MODEL_DISPATCH_FAILED";

/// Marker indicating a USD-driven cosim entity has been wired up by
/// `process_usd_cosim_prims`. Prevents the system from re-processing
/// the same entity on the same tick.
#[derive(Component, Default)]
pub struct UsdSourcedCosim;

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

/// Single-flight guard for [`LoadScene`]: set the instant a scene load is
/// dispatched, cleared once `sync_usd_visuals` has drained every
/// `UsdAwaitingStage` prim for that scene's stage asset.
///
/// **Why.** Two independent triggers fire `LoadScene` on web startup —
/// the boot policy's `StartTutorial` (which `load_scene`s its own
/// environment) and the page's `autoloadDefaultScene` hook (which
/// `LoadScene`s the deploy default, e.g. moonbase). On a first run
/// both land in the same event-loop window. Without a guard, the
/// second `LoadScene`'s cleanup despawns the first scene's prims while
/// `sync_usd_visuals` still has deferred writes queued for them → the
/// "Entity despawned" panic that aborts wasm (the `try_insert` patch
/// above makes that a quiet no-op, but the deeper fix is to prevent the
/// second load from firing at all while the first is still spawning).
///
/// **Policy: first in-flight load wins.** A `LoadScene` arriving while
/// this guard holds a *different* path is suppressed (log + no-op). The
/// tutorial's `load_scene` runs during `Startup`, the page autoload
/// runs after the first frame paints — so the tutorial load is queued
/// first and the page autoload is the one suppressed. On a returning
/// run the boot policy stands down (no `StartTutorial`), no load is
/// in-flight by autoload time, and the moonbase autoload proceeds
/// normally. A later user-driven `LoadScene` (picking a different scene
/// in the browser) finds the guard cleared (the prior scene finished
/// spawning) and proceeds via the normal clear+respawn path.
///
/// The guard is keyed by stage `AssetId` (not path string) so the
/// clearing system can match it against `UsdPrimPath::stage_handle.id()`
/// on draining `UsdAwaitingStage` entities.
#[derive(Resource)]
pub struct SceneLoadInFlight {
    /// Asset-relative path of the in-flight scene (informational; logged
    /// on suppression so the console names the losing load).
    pub path: String,
    /// Stage asset id of the in-flight load. The clearing system watches
    /// for the last `UsdAwaitingStage` entity carrying this id to gain
    /// `UsdVisualSynced` (i.e. leave the awaiting pool).
    pub stage_id: bevy::asset::AssetId<UsdStageAsset>,
}

/// Queued Modelica source load. Inserted by `process_usd_cosim_prims`;
/// drained by `dispatch_loaded_modelica_sources` once the
/// `Handle<ModelicaSource>` has resolved to bytes.
#[derive(Component)]
pub struct PendingModelicaSource {
    pub handle: Handle<ModelicaSource>,
    /// Asset-relative path, copied for use as the eventual
    /// `ModelicaModel::model_path` (purely informational once the
    /// source is in memory).
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

/// How long [`SceneLoadInFlight`] may be held before the watchdog in
/// [`clear_scene_load_in_flight`] declares the load lost and drops it.
///
/// 60 s, matching the readiness ticket derived from this same guard and
/// `lunco_terrain_surface`'s `GEN_STATUS_MAX_SECS`. It has to clear a genuine
/// worst case — a cold web fetch of a large stage plus its reference closure —
/// so it is deliberately generous; the guard exists to serialise two loads that
/// arrive in the same event-loop window, not to bound a slow one. A load that
/// really is still working at 60 s has already blown the readiness deadline, so
/// nothing downstream is waiting on this guard by then anyway.
const SCENE_LOAD_MAX_SECS: f64 = 60.0;

/// Clears [`SceneLoadInFlight`] once `sync_usd_visuals` has drained every
/// `UsdAwaitingStage` prim for the in-flight scene's stage — i.e. once the
/// scene's prims have all spawned (or failed to load). After this runs, a
/// later `LoadScene` (e.g. the user picking a different scene in the
/// browser) proceeds via the normal clear+respawn path instead of being
/// suppressed. Runs every `Update` but is a single `is_empty` query when no
/// guard is set.
///
/// At the moment the in-flight guard drops — the single authoritative "scene
/// finished spawning" edge — this also enforces the lighting invariant: a
/// scene that is meant to be seen must provide at least one `DirectionalLight`
/// (a UsdLux `DistantLight`, the celestial bootstrap sun, or both). The sun is
/// scene content, authored in USD like every other light, so an absent sun is a
/// real authoring defect, not a missing default — and it fails LOUD here rather
/// than rendering dark and silent. `bevy_light`'s `DirectionalLight` is
/// render-free, so this check is layer-appropriate in this render-free crate.
///
/// **Give-up path.** Neither intended clear — the drain, or
/// `AssetLoadFailedEvent` via `lunco_usd_bevy::fail_awaiting_stage_prims` — is
/// guaranteed to arrive: a web fetch that 404s can report no failure at all. So
/// the guard also carries a [`SCENE_LOAD_MAX_SECS`] wall-clock deadline, after
/// which it is dropped and the abandonment published as
/// [`lunco_usd_bevy::SCENE_LOAD_FAILED`]. A stuck guard is the worst outcome
/// available here: it suppresses every subsequent `LoadScene`, so the app looks
/// alive and never loads a scene again, with nothing but a log line to say so.
/// The lighting invariant is NOT checked on the watchdog path — a scene that
/// never finished spawning has not earned that verdict.
fn clear_scene_load_in_flight(
    in_flight: Option<Res<SceneLoadInFlight>>,
    q_awaiting: Query<&UsdPrimPath, With<UsdAwaitingStage>>,
    q_lights: Query<&bevy::light::DirectionalLight>,
    // REAL time, deliberately, and for the same reason
    // `lunco_api::executor::expire_deferred_requests` uses it: a paused or
    // time-warped simulation must not change when a load is declared lost.
    time: Res<Time<bevy::time::Real>>,
    // Deadline for the guard currently held, re-armed whenever the in-flight
    // stage changes. A `Local` rather than a field on `SceneLoadInFlight`
    // because the resource is constructed elsewhere (`lunco_usd::commands`) and
    // its shape is public.
    mut deadline: Local<Option<(bevy::asset::AssetId<UsdStageAsset>, f64)>>,
    mut commands: Commands,
) {
    let Some(g) = in_flight else {
        // No guard held — forget any deadline so the next load arms a fresh one.
        *deadline = None;
        return;
    };
    let now = time.elapsed_secs_f64();
    let expires = match *deadline {
        Some((stage, at)) if stage == g.stage_id => at,
        // First frame of this load (or a different scene took the guard).
        _ => {
            let at = now + SCENE_LOAD_MAX_SECS;
            *deadline = Some((g.stage_id, at));
            at
        }
    };
    // Still spawning if any prim tagged for this stage hasn't been
    // processed by `sync_usd_visuals` (i.e. still carries
    // `UsdAwaitingStage`).
    let still_awaiting = q_awaiting
        .iter()
        .any(|upp| upp.stage_handle.id() == g.stage_id);
    if still_awaiting {
        if now < expires {
            return;
        }
        // WATCHDOG. The two intended clear paths are "every awaiting prim
        // drained" and `AssetLoadFailedEvent` — and on web a 404 can report
        // neither (see `lunco_usd_bevy::fail_awaiting_stage_prims`). Left alone
        // the guard is permanent, and a permanent guard SILENTLY suppresses
        // every later `LoadScene`: the app looks alive and simply never loads a
        // scene again. Dropping it is strictly better than that — a second
        // `LoadScene` can at worst repeat the failure, loudly.
        error!(
            "[scene] `{}` never finished spawning within {SCENE_LOAD_MAX_SECS:.0}s — \
             abandoning the in-flight guard so later scene loads are not suppressed. \
             Prims still awaiting this stage will never instantiate; the stage asset \
             most likely failed to load without reporting a failure (a web 404).",
            g.path
        );
        commands.trigger(lunco_core::TelemetryEvent {
            name: lunco_usd_bevy::SCENE_LOAD_FAILED.into(),
            source: 0,
            severity: lunco_core::Severity::Error,
            data: lunco_core::TelemetryValue::String(g.path.clone()),
            timestamp: 0.0,
        });
        *deadline = None;
        commands.remove_resource::<SceneLoadInFlight>();
        return;
    }
    *deadline = None;
    // Scene fully spawned — enforce the lighting invariant before dropping the
    // guard. The celestial bootstrap sun (`lunco_celestial::big_space_setup`)
    // spawns on site-anchor detection during this same load window and is a
    // `DirectionalLight`, so its presence satisfies the check; only a scene
    // that authors NEITHER a `DistantLight` NOR triggers the celestial sun is
    // flagged.
    if q_lights.is_empty() {
        error!(
            "[scene] `{}` finished loading with no DirectionalLight — the scene \
             will render dark. Author a UsdLux `DistantLight` (the sun) in the \
             scene, or ensure a celestial site anchor is present so the solar \
             bootstrap provides one. There is no Rust fallback sun: scene \
             lighting is scene content.",
            g.path
        );
    }
    commands.remove_resource::<SceneLoadInFlight>();
}

pub fn process_usd_cosim_prims(
    mut commands: Commands,
    query: Query<(Entity, &UsdPrimPath), Without<UsdSourcedCosim>>,
    stages: Res<Assets<UsdStageAsset>>,
    // Read the LIVE canonical stage (source of truth), built on demand from
    // the asset's recipe.
    mut canonical: NonSendMut<CanonicalStages>,
    asset_server: Res<AssetServer>,
    mut wiring_dirty: ResMut<WiringDirty>,
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
            lunco_usd_bevy::program::network_member_paths(&view)
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
        );
    }
}

/// Project the standard LunCo telemetry declaration attributes into the shared
/// telemetry sampler. The declaration stays in USD; this is only the runtime
/// projection, so descriptions and units remain authored data all the way to
/// the signal registry.
fn project_usd_telemetry(
    mut commands: Commands,
    query: Query<(Entity, &UsdPrimPath), Without<UsdTelemetryProjected>>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<CanonicalStages>,
) {
    for (entity, prim_path) in &query {
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
            commands.entity(entity).try_insert(UsdTelemetryProjected);
            continue;
        };
        let view = stage.view();
        let authored = view
            .scalar::<bool>(&path, "lunco:telemetry")
            .unwrap_or(false);
        if authored {
            let port = view.text(&path, "lunco:telemetry:port");
            let name = view.text(&path, "lunco:telemetry:name");
            if let (Some(port), Some(name)) = (
                port.filter(|p| !p.is_empty()),
                name.filter(|n| !n.is_empty()),
            ) {
                commands.spawn((
                    Name::new(format!("telemetry:{name}")),
                    ChildOf(entity),
                    Parameter {
                        name,
                        unit: view.text(&path, "lunco:telemetry:unit").unwrap_or_default(),
                        description: view.text(&path, "lunco:telemetry:description"),
                        source: ChannelSource::Port(port),
                        target: Some(entity),
                        ..default()
                    },
                ));
            }
        }
        commands.entity(entity).try_insert(UsdTelemetryProjected);
    }
}

/// Reads one cosim prim's attributes and dispatches its model + wires + events,
/// generic over the read source ([`UsdRead`]) — drives off either the live
/// canonical `StageView` or the flattened `sdf::Data`, identically.
fn process_usd_cosim_prim_read(
    reader: &lunco_usd_bevy::StageView<'_>,
    entity: Entity,
    prim_path: &UsdPrimPath,
    sdf_path: &SdfPath,
    // Every prim some `CollectionAPI:components` scope on this stage owns.
    network_members: &BTreeSet<String>,
    commands: &mut Commands,
    asset_server: &AssetServer,
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
        let Some(name) = reader.text(sdf_path, "lunco:event:name") else {
            warn!(
                "[usd-cosim] {}: LunCoEvent has no lunco:event:name",
                sdf_path
            );
            return;
        };
        commands.entity(entity).try_insert(EventBinding {
            source_path: source_path.to_string(),
            output: output.to_string(),
            name,
            severity: parse_event_severity(
                reader
                    .text(sdf_path, "lunco:event:severity")
                    .as_deref()
                    .unwrap_or("info"),
            ),
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
    // …and the converse. A part with acausal pins that NO network owns cannot be
    // solved at all: its `.mo` is a component class whose pins only mean
    // something inside a `connect()` set, so there is nothing to run standalone.
    // Silence here is how a battery dropped into a rover with no `Electrical`
    // scope simply fails to exist — the pin reads as connected in USD while the
    // circuit it belongs to was never generated.
    if reader
        .attr_names(sdf_path)
        .iter()
        .any(|name| name.starts_with("connectors:"))
    {
        warn!(
            "[usd-cosim] {}: declares acausal `connectors:*` but belongs to no \
             CollectionAPI:components network, so no Modelica model is generated for it and it \
             does not simulate. Add it to a network scope's `collection:components:includes`.",
            prim_path.path
        );
        commands.entity(entity).try_insert(UsdSimProcessed);
        return;
    }

    // Active-cosim gate: a prim is stepped iff it BOTH binds a behavior model
    // AND declares connectable ports (`inputs:`/`outputs:` attributes). The two
    // non-active cases skip silently: a model with no ports is a
    // documentation-only reference (wheels/motors/batteries carry
    // `lunco:modelicaModel` for provenance); ports with no model are a pure
    // physics sink driven through its backend (a joint receiving
    // `inputs:angle`, a rigid body receiving `inputs:force_y`). Wiring itself
    // is native `connectionPaths`, derived by `rewire_usd_connections`
    // (the journaled, distributed path), never parsed here.
    // A program names its source as an `asset`. The LANGUAGE comes from the file's
    // extension, never from a second attribute: the same `.py` is a plant on one
    // prim and a script on the next, so a `lunco:pythonModel`-style name would be
    // asserting a role the file does not have. This is how USD itself dispatches
    // `.usda` / `.usdc` / `.usdz`.
    let source = reader.asset(sdf_path, "info:sourceAsset");
    let (modelica_path, python_path) = match source.as_deref().map(solver_language) {
        Some(Some(SolverLanguage::Modelica)) => (source.clone(), None),
        Some(Some(SolverLanguage::Python)) => (None, source.clone()),
        // A program this crate does not solve (a `.rhai` script, a `.xml` tree).
        // It is somebody else's to run; it is not a cosim model.
        Some(None) => return,
        None => return,
    };
    let has_ports = reader
        .attr_names(sdf_path)
        .iter()
        .any(|n| n.starts_with("inputs:") || n.starts_with("outputs:"));
    if !has_ports {
        return;
    }

    // `UsdSourcedCosim` already inserted above; add the cosim-only markers.
    //
    // NB: this stamps `UsdSimProcessed`, which makes `process_usd_sim_prims` skip this
    // prim — fine, because link/celestial projection is now its OWN system
    // (`project_celestial_comms_prims`), gated by its OWN marker, so a cosim antenna
    // still gets its `LinkNode`. The two concerns no longer race on one flag.
    commands
        .entity(entity)
        .try_insert((UsdSimProcessed, lunco_core::SelectableRoot));

    // NOTE: there is no possessable/vessel tag to stamp — possession is not gated by
    // a marker at all (an avatar may possess anything; WHO may hold it is the
    // authority layer's call). A prim's command CAPABILITY comes from its `Controls`
    // scope → `ControlBinding` + `InputPorts`, stamped in the general USD
    // translator (`lunco-usd-bevy`), which runs for every prim — not here, which only
    // sees model-bound cosim prims. A lander's actuation backend is its
    // `SimComponent` manual-override ports (written by `SetPorts`), read
    // by topology at possess/route time.

    // Opaque-body guard, applied HERE (cosim intent is known the instant we
    // read `lunco:modelicaModel`/`lunco:pythonModel`) rather than only later
    // in `tag_cosim_opaque`, which waits for the asynchronously-wrapped
    // `SimComponent`. That async gap was a prediction-takeover race: on a
    // client, `maintain_predicted_dynamic` (sandbox-edit) could stamp a balloon
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
    let (inputs, outputs) = declared_interface(reader, sdf_path);
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
        // Unreachable: `solver_language` above returned early for anything that is
        // neither. Kept total so a new language can't silently skip publication.
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
    if reader
        .scalar::<bool>(sdf_path, "lunco:program:realtimeSafe")
        .unwrap_or(false)
    {
        commands
            .entity(entity)
            .try_insert(lunco_cosim::RealtimeSafe);
    }

    info!(
        "[usd-cosim] program {} bound ({})",
        prim_path.path,
        source.as_deref().unwrap_or("<none>"),
    );
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

/// The languages this crate can put a solver behind. Everything else is a program
/// somebody else runs — the rhai engine, the behaviour-tree compiler — and this
/// crate leaves it alone.
enum SolverLanguage {
    Modelica,
    Python,
}

/// Which solver, if any, runs a program — decided by its file's extension, exactly
/// as USD picks a file-format plugin by `.usda` / `.usdc` / `.usdz`. `None` is not
/// an error: it is a program with a different engine behind it.
fn solver_language(path: &str) -> Option<SolverLanguage> {
    match path.rsplit_once('.').map(|(_, ext)| ext) {
        Some("mo") => Some(SolverLanguage::Modelica),
        Some("py") => Some(SolverLanguage::Python),
        _ => None,
    }
}

/// Drain `PendingModelicaSource` for entities whose `.mo` text has
/// finished loading via `AssetServer`. Parses the source, populates a
/// `ModelicaModel` stub, dispatches `ModelicaCommand::Compile`, and
/// removes the pending marker. Stable retry behaviour: if the asset
/// isn't ready this frame we just skip — the system runs again next
/// frame.
pub fn dispatch_loaded_modelica_sources(
    mut commands: Commands,
    mut q: Query<(
        Entity,
        &PendingModelicaSource,
        &UsdPrimPath,
        &mut SimComponent,
    )>,
    sources: Res<Assets<ModelicaSource>>,
    asset_server: Res<AssetServer>,
    canonical: NonSend<CanonicalStages>,
    channels: Option<Res<ModelicaChannels>>,
    mut notices: MessageWriter<lunco_modelica::ModelicaNotice>,
    // Half of the solver-selection input, and the half only the ECS knows: the
    // DECLARED `lunco:program:realtimeSafe` promise. The worker derives the other
    // half (does the model carry algebraic unknowns) from the compiled DAE.
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
    pending.sort_unstable_by(|(_, _, a, _), (_, _, b, _)| a.path.cmp(&b.path));

    for (entity, pending, prim_path, mut component) in pending {
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
            commands.entity(entity).remove::<PendingModelicaSource>();
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
        let parameters = interface.parameters;
        let inputs = interface.inputs;
        let usd_documentation = canonical
            .get(prim_path.stage_handle.id())
            .and_then(|stage| {
                let path = SdfPath::new(&prim_path.path).ok()?;
                stage.view().documentation(&path)
            });
        let outputs = interface
            .variable_metadata
            .into_iter()
            .map(|(name, metadata)| {
                let provenance = if metadata.description.is_some() {
                    "modelica"
                } else if usd_documentation.is_some() {
                    "usd"
                } else {
                    "modelica"
                };
                (
                    name,
                    CosimOutputDescriptor {
                        description: metadata.description.or_else(|| usd_documentation.clone()),
                        unit: metadata.unit,
                        provenance: provenance.to_string(),
                        canonical_name: None,
                        group_path: None,
                    },
                )
            })
            .collect();

        // DISPATCH FIRST, then stub. NOT `let _ = send(..)`: a closed worker
        // channel means the compile is never attempted, and a `ModelicaModel`
        // with no `last_error` and no `variables` projects `SimStatus::Compiling`
        // *every tick* through `sync_modelica_outputs`/`modelica_status` — a
        // state nothing can move it out of, so the model silently never steps.
        // The failure therefore has to live on the MODEL (`last_error`), not on
        // the component, or the next tick overwrites it. Closed-channel
        // detection is `send(..).is_err()`, the same test
        // `source_roots::ensure_loaded` uses.
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
                stream: None,
                // Declared, never inferred. A program without the promise is not
                // client-predicted, so an adaptive implicit solver is correct for
                // it — which is exactly what a battery/solar electrical island
                // needs.
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

        commands
            .entity(entity)
            .try_insert(CosimOutputMetadata { outputs });
        commands.entity(entity).try_insert(ModelicaModel {
            model_path: PathBuf::from(&pending.asset_path),
            model_name: model_name.clone(),
            parameters,
            inputs,
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

        commands.entity(entity).remove::<PendingModelicaSource>();
    }
}

/// Drain `PendingPythonSource` analogously to the Modelica version.
pub fn dispatch_loaded_python_sources(
    mut commands: Commands,
    q: Query<(Entity, &PendingPythonSource)>,
    sources: Res<Assets<PythonSource>>,
    asset_server: Res<AssetServer>,
    mut registry: ResMut<ScriptRegistry>,
    // The `SimComponent` was published at BIND with the USD-declared interface;
    // dispatch reads it to seed the editor document and flips it live.
    mut sims: Query<&mut SimComponent>,
) {
    for (entity, pending) in q.iter() {
        if asset_server.load_state(&pending.handle).is_failed() {
            warn!(
                "[usd-cosim] failed to load Python source `{}` via AssetServer",
                pending.asset_path
            );
            commands.entity(entity).remove::<PendingPythonSource>();
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

        // Offset doc id away from any Modelica-allocated ids on the same
        // entity (legacy catalog Python balloon does the same).
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
        commands.entity(entity).try_insert(ScriptedModel {
            document_id: Some(doc_id.raw()),
            language: Some(ScriptLanguage::Python),
            paused: false,
            inputs: Default::default(),
            outputs: Default::default(),
        });

        // Script loaded: flip the bind-published `SimComponent` live. It was
        // created `Compiling` at bind carrying the USD-declared interface — do NOT
        // re-create it here (that discarded the interface and made every wire into
        // this model false-warn as an unknown input). Python has no separate
        // compile step, so loaded ⇒ `Running`.
        if let Ok(mut sim) = sims.get_mut(entity) {
            sim.status = SimStatus::Running;
        }

        commands.entity(entity).remove::<PendingPythonSource>();
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
pub fn wrap_modelica_into_simcomponent(
    mut commands: Commands,
    q_new: Query<(Entity, &ModelicaModel), (With<UsdSourcedCosim>, Without<SimComponent>)>,
) {
    for (entity, model) in q_new.iter() {
        commands.entity(entity).try_insert(SimComponent {
            model_name: model.model_name.clone(),
            parameters: model.parameters.clone(),
            inputs: model.inputs.clone(),
            // Outputs are the SOLUTION — empty until the worker answers.
            // `sync_modelica_outputs` fills them and flips the status.
            outputs: model.variables.clone(),
            status: modelica_status(model),
            is_stepping: model.is_stepping,
        });
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
    } else if !model.is_compiled || model.current_time <= 0.0 || model.variables.is_empty() {
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
    mut q: Query<
        (
            &ModelicaModel,
            &mut SimComponent,
            Option<&GeneratedModelicaSource>,
            Option<&mut CosimOutputMetadata>,
        ),
        With<UsdSourcedCosim>,
    >,
) {
    for (model, mut comp, generated, mut metadata) in &mut q {
        upsert_ports(&mut comp.outputs, model.variables.iter());
        for (k, v) in &model.inputs {
            comp.inputs.entry(k.clone()).or_insert(*v);
        }
        if let (Some(source), Some(metadata)) = (generated, metadata.as_deref_mut()) {
            for name in model.variables.keys() {
                let descriptor =
                    metadata
                        .outputs
                        .entry(name.clone())
                        .or_insert_with(|| CosimOutputDescriptor {
                            description: None,
                            unit: None,
                            provenance: "modelica".to_string(),
                            canonical_name: None,
                            group_path: None,
                        });
                // The wrapper topology is immutable between projection
                // revisions. Resolve each newly observed solver variable once;
                // steady-state ticks only copy values into SimComponent.
                if descriptor.group_path.is_none() {
                    descriptor.group_path =
                        crate::domain_projection::generated_signal_group(source, name);
                }
                if descriptor.canonical_name.is_some() {
                    continue;
                }
                descriptor.canonical_name =
                    crate::domain_projection::canonical_generated_signal(source, name);
            }
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
fn copy_modelica_input_values(model: &mut ModelicaModel, component: &SimComponent) {
    for (name, value) in &component.inputs {
        if model.inputs.contains_key(name) || model.compiled_input_names.contains(name) {
            model.inputs.insert(name.clone(), *value);
        }
    }
}

/// Per-tick: SimComponent.inputs → ModelicaModel.inputs.
/// Hands wire-propagated values (height, velocity, …) back to the
/// Modelica worker for the next solver step.
pub fn sync_modelica_inputs(
    mut q: Query<(&SimComponent, &mut ModelicaModel), With<UsdSourcedCosim>>,
) {
    for (comp, mut model) in &mut q {
        copy_modelica_input_values(&mut model, comp);
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
    armed: bool,
}

fn parse_event_severity(value: &str) -> lunco_core::Severity {
    match value {
        "debug" => lunco_core::Severity::Debug,
        "warning" => lunco_core::Severity::Warning,
        "error" => lunco_core::Severity::Error,
        "critical" => lunco_core::Severity::Critical,
        _ => lunco_core::Severity::Info,
    }
}

fn event_rising_edge(armed: &mut bool, value: f64) -> bool {
    let active = value >= 0.5;
    if active && *armed {
        *armed = false;
        true
    } else {
        if !active {
            *armed = true;
        }
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
    mut commands: Commands,
) {
    // Nothing is listening: don't index the scene. This runs every FixedUpdate
    // tick and the index below is a full scan of every cosim participant plus a
    // fresh allocation — paid on every scene, while `LunCoEvent` prims are rare.
    if bindings.is_empty() {
        return;
    }
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
        if event_rising_edge(&mut binding.armed, value) {
            commands.trigger(lunco_core::TelemetryEvent {
                name: binding.name.clone(),
                source,
                severity: binding.severity,
                data: lunco_core::TelemetryValue::F64(value),
                timestamp: 0.0,
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
        (Some(_), Some(component)) => !matches!(component.status, SimStatus::Compiling),
        (Some(_), None) => false,
    })
}

/// Native physics endpoints must be admitted before a dynamic body is allowed
/// to integrate. Modelica compilation is entity-scoped and therefore does not
/// hold the whole world, but a pending joint/wheel/reference endpoint is a
/// world-level safety condition: otherwise the body can fall or drift before
/// its constraint exists and Avian later seats it with a large violation.
/// The sole USD-side transition from a loading projection epoch to a bindable
/// one.  Failed models are terminal: readiness policy decides whether to hold
/// physics, while the binder must be allowed to record the failed endpoint.
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
    // and defeats entity-scoped readiness. The world ticket is only for native
    // endpoint settlement after every Modelica participant is terminal.
    let hold_binding_epoch = !settled && models_terminal && !connections.is_empty();
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
/// **Why this table exists at all.** A USD connection into a structural binding is real
/// and correct USD; what it is *not* is a value the runtime propagates. Nothing ever
/// registers a backend for these ports, so materialising them as `SimConnection`s leaves
/// targets no backend claims. Propagation then reports each one as a genuine dangling
/// wire — genuine because these prims *do* expose other ports, so `has_port_surface` is
/// true, which is precisely the discriminator the diagnostic uses to separate "typo'd"
/// from "still loading". The result is a permanent, self-confirming false fault on
/// hardware that works.
///
/// That failure mode has now cost two separate investigations: it produced the Apollo-15
/// report's critical misdiagnosis (`torque` read as "the rover's drive authority is
/// dropped") and, once the never-landed gate started reading faults as a test verdict, it
/// failed four scenes — `autopilot_hold`, `six_independent_parity`, `six_wheel` and
/// `lint_selftest` — whose rovers measurably drive.
///
/// **The rule for adding a row:** the port must have a named parse-time reader, and that
/// reader must be cited here. If no code reads it at parse and no backend claims it at
/// runtime, the wire is dangling for real and belongs in the diagnostic, not in this table.
const STRUCTURAL_INPUT_BINDINGS: &[(&str, &str)] = &[
    // `Motor.inputs:demand` is the authored command annotation on a motor
    // instance. The live axle command is consumed by `MotorActuator` through
    // the wheel's resolved drive port; the motor prim itself is folded into
    // `PowertrainParams` by `powertrain::find_for_wheel`. In the infinite-power
    // variant there is no Modelica island to own this USD edge, so materialising
    // it as a runtime SimConnection creates a false dangling-wire fault. The
    // battery variant is handled by domain projection because those motors are
    // collection members of the generated electrical network.
    ("demand", "LunCoMotorAPI"),
    // `Gearbox.inputs:torque ← Motor.outputs:torque`. Read by
    // `powertrain::find_for_wheel`, which folds `stallTorque × ratio × efficiency`
    // into a static `WheelParams`. Live axle torque is written every tick by
    // `MotorActuator`, never through this port.
    ("torque", "LunCoGearboxAPI"),
    // `Wheel.inputs:drive` / `inputs:steer`. Read by `connected_port` in
    // `crate::lib`, which resolves the connection to the FSW port NAME the wheel
    // should subscribe to (`PendingWheelWiring::drive_port_name`). The value then
    // flows through the port registry, not along this edge.
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
            Added<lunco_core::GlobalEntityId>,
        )>,
    >,
    mut removed: RemovedComponents<UsdPrimPath>,
    mut dirty: ResMut<WiringDirty>,
    q_all: Query<(Entity, &UsdPrimPath, Has<ModelicaModel>)>,
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
    // opted out of prediction). Absence of the promise is the dangerous case;
    // absence of the body is the safe one.
    q_realtime_safe: Query<&lunco_cosim::RealtimeSafe>,
    q_predicted_body: Query<&avian3d::prelude::RigidBody, Without<lunco_core::NotPredictable>>,
    q_defaults: Query<&UsdInputDefaults>,
    // A vessel's actuator ports are child `Port` entities, so an `outputs:` forward
    // onto one has to write there, not onto the vessel prim.
    q_actuators: Query<&lunco_core::ActuatorPorts>,
    stages: Res<Assets<UsdStageAsset>>,
    mut canonical: NonSendMut<CanonicalStages>,
) {
    // `Added<ModelicaModel>` is the explicit endpoint-contract transition. This
    // pass no longer relies on accidentally deferred removal events to get an
    // extra rewire after a generated model appears.
    let structural = !wiring_arrivals.is_empty() || removed.read().next().is_some();
    if !structural && !dirty.0 {
        return;
    }
    dirty.0 = false;

    // A prim's instance identity (its instance-root GID, `None` for scene prims)
    // is what keeps two spawns of one asset — byte-identical stage-relative paths
    // and all — from collapsing onto one entity below. See `instance_key`.
    let instance_of =
        |e: Entity| lunco_usd_bevy::instance_key(e, &q_provenance, &q_gid, &q_instance_root);

    // Index every prim entity by (instance, path). Keying on the instance is what
    // keeps two spawns of one asset distinct: their identical stage-relative paths
    // now land under different instance keys instead of overwriting each other.
    let mut by_path: HashMap<(Option<u64>, String), Entity> = HashMap::new();
    for (e, p, _) in q_all.iter() {
        by_path.insert((instance_of(e), p.path.clone()), e);
    }

    // Authored constants on unconnected `inputs:` ports — a model's parameters.
    // Gathered in the same sweep that derives the wires, because "has no wire" is
    // exactly what makes an input a parameter.
    let mut defaults: HashMap<Entity, HashMap<String, f64>> = HashMap::new();

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

    for (entity, prim_path, has_modelica) in q_all.iter() {
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
            .or_insert_with(|| lunco_usd_bevy::program::network_member_paths(&view));
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
            // (`ActuatorPorts`, one `value` scalar each), which is where the write
            // has to land; anything else writes the name on the prim itself.
            //
            // One hop per authored forward, so a chain resolves as a chain of edges
            // — no walk, and no second resolution path for consumers to disagree
            // about. This is the only reader of output connections: before it,
            // `outputs:*.connect` was authored in three drive-law overlays and in
            // `skid_rover.usda`'s `Electrical` scope and did nothing at all, which is
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
            let forward =
                attr.strip_prefix("outputs:").filter(|_| !shading_prim).map(
                    |name| match q_actuators.get(entity).ok().and_then(|a| a.get(name)) {
                        Some(port_entity) => (port_entity, lunco_cosim::PORT_NAME.to_string()),
                        None => (entity, name.to_string()),
                    },
                );
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
            if is_structural_binding(&view, &sink_sdf, sink_conn) {
                continue;
            }
            // Same reasoning, one level up — but for `outputs:` ONLY.
            //
            // An `outputs:` connection authored on a domain network root is read
            // at parse time by `domain_projection` and becomes an equation inside
            // the generated model (`soc = <battery>.soc_out;`). Its source prim is
            // a MEMBER of that island with no `SimComponent` of its own, so a
            // runtime wire could never fire. MEASURED: the electrical islands'
            // `outputs:soc` / `outputs:solar_power` were reported as five
            // connections that "never landed" on a scene whose islands were
            // stepping and publishing those very ports.
            //
            // ⚠ NOT `inputs:`. A network root's `inputs:` are the island's
            // BOUNDARY — `read_network` declares them `input Real` on the
            // generated model and something OUTSIDE must drive them, which is
            // exactly a runtime wire. `rocker_bogie.usda` authors
            // `Electrical.inputs:drive_left.connect = </RockerBogie.outputs:drive_left>`;
            // skipping that would leave the island's demand inputs permanently
            // unwritten and every motor's electrical draw at zero.
            if attr.starts_with("outputs:")
                && crate::domain_projection::is_domain_network_root(&view, &sink_sdf)
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
                && !has_modelica
            {
                continue;
            }
            // SSP `LinearTransformation`: the propagated value is `src * factor +
            // offset`. Authored on the sink prim, keyed by the consuming port
            // (`lunco:factor:<port>` / `:offset:<port>`), so each input carries its
            // own scaling. Absent ⇒ identity (1, 0), matching the pre-migration
            // `lunco:scale` default. The transform is invariant across the fan-in
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
                let Some(&start_element) = by_path.get(&(sink_instance, src_prim.to_string()))
                else {
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

                // ── The SOURCE side of the actuator-port indirection ─────────
                // A vessel's `outputs:drive_left` is not stored on the vessel
                // prim: `ActuatorPorts` realises it as a child `Port` entity, and
                // that is where `apply_drive_mix` writes. The sink side above has
                // always redirected onto that child; reading one had no such hop,
                // so a wire whose SOURCE is a vessel actuator port resolved to the
                // vessel entity, found no port of that name, and delivered its
                // default forever.
                //
                // MEASURED on `scenes/tests/solar_domain_nested_ref.usda`: the
                // rover's `throttle` reached 1.0 and the skid kernel wrote both
                // bank ports, while `Electrical.inputs:drive_left` — wired from
                // `</RockerBogie.outputs:drive_left>` — stayed at 0.0 for the whole
                // run. Every motor drew no current, so a driving rover's battery
                // never discharged and its bus was solved as if parked. Silent:
                // the island compiled, published, and stepped.
                let (start_element, src_conn) = match q_actuators
                    .get(start_element)
                    .ok()
                    .filter(|_| !start_is_input)
                    .and_then(|a| a.get(src_conn))
                {
                    Some(port_entity) => (port_entity, lunco_cosim::PORT_NAME),
                    None => (start_element, src_conn),
                };

                // ── The realtime gate ───────────────────────────────────────
                // A program may only push a client-predicted `Dynamic` body around
                // if it PROMISED it steps fast enough
                // (`lunco:program:realtimeSafe = true`). Without that promise — the
                // common case, since the default is `false` — an adaptive,
                // variable-cost solver is deciding the forces inside the prediction
                // loop, and the body diverges from the server every frame the solver
                // runs late.
                //
                // Warn, don't refuse: cosim prims are stamped `NotPredictable` at
                // prim-read time, so a scene that trips this gate has ALREADY
                // routed around the guard some other way, and dropping the wire
                // silently would leave a vehicle with no forces at all. The warn
                // names the attribute and the prim so it is actionable.
                if lunco_cosim::is_physics_force_port(sink_conn)
                    && matches!(
                        q_predicted_body.get(entity),
                        Ok(avian3d::prelude::RigidBody::Dynamic)
                    )
                    && q_realtime_safe.get(start_element).is_err()
                {
                    warn!(
                        "[usd-cosim] {}.{}: the program at {} drives a force/torque port on a \
                         CLIENT-PREDICTED dynamic body without declaring \
                         `lunco:program:realtimeSafe = true`. Its step sequence and cost are not \
                         guaranteed identical across peers — the predicted body can diverge. \
                         Declare it on the program prim (see \
                         docs/architecture/28-modelica-realtime-physics.md).",
                        prim_path.path, attr, src_prim,
                    );
                }

                let (end_element, end_connector) = forward
                    .clone()
                    .unwrap_or_else(|| (entity, sink_conn.to_string()));
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
pub fn seed_usd_input_defaults(
    mut q: Query<
        (&UsdInputDefaults, &mut SimComponent, &UsdPrimPath),
        Or<(Added<SimComponent>, Changed<UsdInputDefaults>)>,
    >,
) {
    for (defaults, mut sim, prim_path) in q.iter_mut() {
        for (port, value) in &defaults.0 {
            if sim.inputs.contains_key(port) {
                sim.inputs.insert(port.clone(), *value);
            } else {
                warn!(
                    "[usd-cosim] {}: `inputs:{}` is authored but the model ({}) declares no such \
                     input — the value is ignored. Check the port name against the model.",
                    prim_path.path, port, sim.model_name,
                );
            }
        }
    }
}

// ── Uniform port commands (ListPorts / GetPort / SetPort) ───────────────────
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
fn resolve_param_entity(world: &mut World, params: &serde_json::Value) -> Option<Entity> {
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
    fn execute(&self, world: &mut World, params: &serde_json::Value) -> lunco_api::ApiResponse {
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
    fn execute(&self, world: &mut World, params: &serde_json::Value) -> lunco_api::ApiResponse {
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

/// `SetPort` — hold one input port at a setpoint.
///
/// `curl … {"type":"ExecuteCommand","command":"SetPort","params":{"api_id":N,"name":"angle","value":1.2}}`
/// `curl … {"type":"ExecuteCommand","command":"SetPort","params":{"api_id":N,"name":"angle","value":1.2,"hold_secs":30}}`
///
/// The write is a HOLD, not a poke: it outranks the wiring fabric for its
/// duration ([`lunco_cosim::PortHolds`]). Writing the slot alone worked only on
/// an UNWIRED port — on a wired one the next propagation tick overwrote it
/// within 16 ms, so the call reported success and nothing moved, which reads
/// exactly like a port that does not exist.
///
/// Holds are latest-wins per `(entity, port)` and expire ([`lunco_cosim::DEFAULT_HOLD_SECS`]
/// unless `hold_secs` says otherwise), so a stream of setpoints keeps control
/// while an abandoned one hands the port back to its wiring instead of leaving a
/// vehicle stuck. `ReleasePort` ends one early.
pub struct SetPortProvider;

impl lunco_api::ApiQueryProvider for SetPortProvider {
    fn name(&self) -> &'static str {
        "SetPort"
    }
    fn execute(&self, world: &mut World, params: &serde_json::Value) -> lunco_api::ApiResponse {
        let Some(e) = resolve_param_entity(world, params) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::EntityNotFound,
                "SetPort requires a resolvable `api_id`",
            );
        };
        let Some(name) = params.get("name").and_then(|v| v.as_str()) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::DeserializationError,
                "SetPort requires a `name`",
            );
        };
        let Some(value) = params.get("value").and_then(|v| v.as_f64()) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::DeserializationError,
                "SetPort requires a numeric `value`",
            );
        };
        let hold_secs = params
            .get("hold_secs")
            .and_then(|v| v.as_f64())
            .filter(|secs| *secs > 0.0)
            .unwrap_or(lunco_cosim::DEFAULT_HOLD_SECS);
        let ports_reg = world.resource::<lunco_core::ports::PortRegistry>().clone();
        if !ports_reg.write_port(world, e, name, value) {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::DeserializationError,
                format!("no writable input port `{}` on entity", name),
            );
        }
        let now = world
            .get_resource::<Time<bevy::time::Real>>()
            .map(|time| time.elapsed_secs_f64())
            .unwrap_or(0.0);
        if let Some(mut holds) = world.get_resource_mut::<lunco_cosim::PortHolds>() {
            holds.hold(e, name, value, now + hold_secs);
        }
        lunco_api::ApiResponse::ok(
            serde_json::json!({ "name": name, "value": value, "hold_secs": hold_secs }),
        )
    }
}

/// `ReleasePort` — end a [`SetPortProvider`] hold early, handing the port back to
/// whatever drives it.
///
/// `curl … {"type":"ExecuteCommand","command":"ReleasePort","params":{"api_id":N,"name":"angle"}}`
///
/// Holds expire on their own, so this is for the caller who is DONE rather than
/// the caller who crashed: releasing a throttle at the end of a manoeuvre returns
/// the vessel to its autopilot on the next tick instead of after the timeout.
pub struct ReleasePortProvider;

impl lunco_api::ApiQueryProvider for ReleasePortProvider {
    fn name(&self) -> &'static str {
        "ReleasePort"
    }
    fn execute(&self, world: &mut World, params: &serde_json::Value) -> lunco_api::ApiResponse {
        let Some(e) = resolve_param_entity(world, params) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::EntityNotFound,
                "ReleasePort requires a resolvable `api_id`",
            );
        };
        let Some(name) = params.get("name").and_then(|v| v.as_str()) else {
            return lunco_api::ApiResponse::error(
                lunco_api::ApiErrorCode::DeserializationError,
                "ReleasePort requires a `name`",
            );
        };
        let released = world
            .get_resource_mut::<lunco_cosim::PortHolds>()
            .is_some_and(|mut holds| holds.release(e, name));
        lunco_api::ApiResponse::ok(serde_json::json!({ "name": name, "released": released }))
    }
}

/// API query provider: `curl … {"type":"ExecuteCommand","command":"CosimStatus","params":{}}`
/// returns one row per USD-driven cosim entity with position, model
/// state, and propagated cosim values. Lets you probe the running
/// binary without polling logs.
pub struct CosimStatusProvider;

impl lunco_api::ApiQueryProvider for CosimStatusProvider {
    fn name(&self) -> &'static str {
        "CosimStatus"
    }
    fn execute(&self, world: &mut World, _params: &serde_json::Value) -> lunco_api::ApiResponse {
        let mut q = world.query_filtered::<(
            &Name,
            &Transform,
            Option<&SimComponent>,
            Option<&ModelicaModel>,
            Option<&avian3d::prelude::LinearVelocity>,
        ), With<UsdSourcedCosim>>();

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
                    "modelica_var_count": model.map(|m| m.variables.len()).unwrap_or(0),
                    "modelica_paused": model.map(|m| m.paused).unwrap_or(false),
                    "modelica_current_time": model.map(|m| m.current_time).unwrap_or(0.0),
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
        lunco_api::ApiResponse::ok(serde_json::json!({ "entities": entities }))
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

    fn execute(&self, world: &mut World, _params: &serde_json::Value) -> lunco_api::ApiResponse {
        let awaiting = world
            .query_filtered::<&UsdPrimPath, With<UsdAwaitingStage>>()
            .iter(world)
            .map(|path| path.path.clone())
            .collect::<Vec<_>>();
        let pending_joints = world
            .query_filtered::<(
                Entity,
                &UsdPrimPath,
                &lunco_usd_avian::PendingUsdJoint,
                Option<&lunco_core::Provenance>,
                Option<&lunco_core::GlobalEntityId>,
                Has<UsdInstanceRoot>,
            ), With<lunco_usd_avian::PendingUsdJoint>>()
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
        let pending_wheels = world
            .query_filtered::<&UsdPrimPath, With<crate::PendingWheelWiring>>()
            .iter(world)
            .map(|path| path.path.clone())
            .collect::<Vec<_>>();
        let pending_differentials = world
            .query_filtered::<&UsdPrimPath, With<crate::PendingDifferential>>()
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
        let mut bodies = world.query::<(
            Entity,
            &UsdPrimPath,
            Option<&avian3d::prelude::RigidBody>,
            Option<&avian3d::prelude::Position>,
            Option<&avian3d::prelude::RigidBodyDisabled>,
            Option<&lunco_usd_avian::big_space_bridge::BridgeShadow>,
            Option<&lunco_core::Provenance>,
            Option<&lunco_core::GlobalEntityId>,
            Has<UsdInstanceRoot>,
        )>();
        let bodies = bodies
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
        let mut models = world.query_filtered::<(
            &Name,
            Option<&ModelicaModel>,
            Option<&SimComponent>,
        ), With<UsdSourcedCosim>>();
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

        let connection_count = world
            .query_filtered::<(), With<SimConnection>>()
            .iter(world)
            .count();
        let pending_avian_joints = serde_json::json!({
            "revolute": world
                .query_filtered::<(), With<lunco_usd_avian::PendingJoint<avian3d::prelude::RevoluteJoint>>>()
                .iter(world)
                .count(),
            "prismatic": world
                .query_filtered::<(), With<lunco_usd_avian::PendingJoint<avian3d::prelude::PrismaticJoint>>>()
                .iter(world)
                .count(),
            "fixed": world
                .query_filtered::<(), With<lunco_usd_avian::PendingJoint<avian3d::prelude::FixedJoint>>>()
                .iter(world)
                .count(),
        });
        let pending_admission_details = world
            .query_filtered::<(
                Entity,
                &lunco_usd_avian::PendingJointAdmission,
                Option<&UsdPrimPath>,
            ), With<lunco_usd_avian::PendingJointAdmission>>()
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
        let admitted_avian_joints = serde_json::json!({
            "revolute": world
                .query_filtered::<(), With<avian3d::prelude::RevoluteJoint>>()
                .iter(world)
                .count(),
            "prismatic": world
                .query_filtered::<(), With<avian3d::prelude::PrismaticJoint>>()
                .iter(world)
                .count(),
            "fixed": world
                .query_filtered::<(), With<avian3d::prelude::FixedJoint>>()
                .iter(world)
                .count(),
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
/// It intentionally includes every Bevy camera, because a camera spawned outside
/// the avatar path must be visible to this audit too.
pub struct SceneCameraAuditProvider;

impl lunco_api::ApiQueryProvider for SceneCameraAuditProvider {
    fn name(&self) -> &'static str {
        "SceneCameraAudit"
    }

    fn execute(&self, world: &mut World, _params: &serde_json::Value) -> lunco_api::ApiResponse {
        let mut query = world.query_filtered::<(
            Entity,
            Option<&Name>,
            Option<&UsdPrimPath>,
            &bevy::camera::Camera,
            Has<SceneCamera>,
            Has<lunco_usd_bevy::camera_mount::MountedCamera>,
            Has<Avatar>,
            Has<LocalAvatar>,
            Has<lunco_avatar::ProvisionalAvatarCamera>,
        ), ()>();
        let mut candidates: Vec<_> = query
            .iter(world)
            .map(
                |(
                    entity,
                    name,
                    prim,
                    camera,
                    scene_camera,
                    mounted,
                    avatar,
                    local,
                    provisional,
                )| {
                    serde_json::json!({
                        "entity": entity.to_bits(),
                        "name": name.map(|n| n.as_str()).unwrap_or_default(),
                        "usd_path": prim.map(|p| p.path.as_str()),
                        "stage": prim.map(|p| format!("{:?}", p.stage_handle.id())),
                        "scene_camera": scene_camera,
                        "mounted_camera": mounted,
                        "avatar": avatar,
                        "local_avatar": local,
                        "provisional_avatar": provisional,
                        "camera_active": camera.is_active,
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
/// `curl … {"type":"ExecuteCommand","command":"LoadScene","params":{"path":"scenes/luncosim/sandbox_scene.usda"}}`
///
/// - `path`: USD asset path relative to the asset root.
/// - `root_prim`: optional override for the SDF path of the prim to
///   spawn. Empty (default) reads the stage's `defaultPrim` metadata;
///   if absent, falls back to `/` (walk all top-level prims).
///
/// Despawns every existing entity carrying `UsdPrimPath` plus every
/// `SimConnection` (cosim wires are scene-derived in current code), then
/// reloads the asset from disk and spawns a fresh root entity. Existing
/// pipelines (`sync_usd_visuals`, `process_usd_cosim_prims`, the
/// avian/sim translators) take it from there. The first `Grid` entity
/// in the world is used as the parent — i.e. the `BigSpace` host
/// stays put across reloads.
///
/// Cleans up worker-side state too: sends `ModelicaCommand::Despawn`
/// for every entity carrying a `ModelicaModel` (the Modelica worker
/// drops its `steppers` / `cached_models` / `sim_streams` entries) and
/// drops `ScriptRegistry::documents` entries for every `ScriptedModel`.
/// Without this, repeated reloads accumulate stale steppers and parsed
/// scripts indefinitely.
#[Command(default)]
pub struct LoadScene {
    /// USD asset path (relative to `assets/`).
    pub path: String,
    /// Optional override for the prim to spawn. Empty (default) reads
    /// `defaultPrim` from the stage's metadata header, falling back to
    /// `/` when none is declared.
    pub root_prim: String,
}

// The `LoadScene` OBSERVER lives in `lunco-usd`
// (`commands.rs::on_load_scene`), not here: mounting a scene has to resolve the
// requested path to its DOCUMENT first (a doc-backed scene must mount its
// composed `base ⊕ runtime`, never the base file), and the document registry
// lives one layer up. This crate owns the mount MECHANICS the observer drives —
// [`normalize_scene_asset_path`], [`resolve_root_prim`], [`clear_scene_entities`],
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
    faults: Option<Res<lunco_core::RuntimeFaults>>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
    q_usd: Query<(Entity, &UsdPrimPath)>,
    scene: SceneEntities,
) {
    if let Some(reason) = faults
        .as_deref()
        .and_then(|faults| faults.scene_mutation_rejection("restart a scene"))
    {
        error!("[restart-scene] {reason}");
        return;
    }
    // Every loaded prim shares the scene's stage handle. REUSE that handle (not a
    // freshly-resolved path) so the exact same asset — INCLUDING its source scheme
    // (`twin://…`, `lunco://…`) — is respawned. Resolving via `.path()` would
    // drop the scheme and load a *different* raw-file asset, breaking twin routing
    // (avatar/camera setup, composed runtime edits) and leaving a stale camera.
    let Some((_, upp)) = q_usd.iter().next() else {
        warn!("[restart-scene] no scene is loaded — nothing to restart");
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
        if let Some(ap) = asset_path {
            world.resource::<AssetServer>().reload(ap);
        }
        spawn_scene_root_with_stage(world, &label, "", handle);
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
fn on_clear_scene(
    trigger: On<ClearScene>,
    faults: Option<Res<lunco_core::RuntimeFaults>>,
    mut commands: Commands,
    scene: SceneEntities,
) {
    if let Some(reason) = faults
        .as_deref()
        .and_then(|faults| faults.scene_mutation_rejection("clear the scene"))
    {
        error!("[clear-scene] {reason}");
        return;
    }
    info!("[clear-scene] clearing viewport");
    clear_scene_entities(&mut commands, &scene);
}

/// Despawn the current scene's USD entities + cosim wires.
/// External/worker state (such as Modelica steppers or Python script documents)
/// is cleaned up automatically via reactive `On<Remove, T>` component observers
/// registered in their respective home crates (`lunco-modelica` and `lunco-scripting`).
/// Shared by [`LoadScene`] (clear-before-reload) and [`ClearScene`]
/// (clear-to-empty). Despawns are deferred through `commands`.
///
/// TODO(avian-bump): this plain batch despawn trips a DEBUG-only assert in avian
/// 0.7 — `island.contact_count == 0` (islands/mod.rs:1372), from
/// `BodyIslandNode::on_remove` when an island's last body leaves while a contact is
/// still registered against it. It is currently silenced by
/// `[profile.dev.package.avian3d] debug-assertions = false` (see the workspace
/// Cargo.toml for the full rationale) — a MASK, not a fix. Verified benign: the
/// island is deleted on the next line, and physics simulates correctly after a
/// reload (rover stays finite and rests on terrain).
///
/// DO NOT "fix" this by reordering the teardown. Every sanctioned order was tried
/// and ALL still panic: remove `RigidBody` then `Collider`; `Collider` then
/// `RigidBody`; `RigidBody` alone; insert `ColliderDisabled` + `RigidBodyDisabled`;
/// gather colliders via `RigidBodyColliders` rather than the Bevy hierarchy; and
/// even stepping `PhysicsSchedule` mid-teardown. Each left islands holding contacts.
/// Root cause is upstream: a collider's contacts drain ONLY on adding
/// `ColliderDisabled`/`Disabled` or REMOVING `ColliderMarker` — and since
/// `ColliderMarker` is a REQUIRED component, dropping `Collider` drains nothing
/// while still unlinking it from `RigidBodyColliders` (which defeats the body's own
/// drain); and `remove_collider_on` early-returns on a non-TOUCHING edge without
/// unlinking it from the island. Re-test on the next avian bump.
///
/// NOTE: any system that touches scene entities through `Commands` must use the
/// FALLIBLE forms (`try_despawn`/`try_remove`/`try_insert`) — its queries are built
/// before this despawn flushes, so its targets can already be dead. A plain
/// `remove`/`insert` panics in `apply_deferred` and takes the app down mid-reload
/// (that was the `sync_gizmo_camera` crash).
/// The scene-owned entities a teardown touches, bundled as one `SystemParam`.
///
/// Every scene-lifecycle observer — `LoadScene` (in `lunco-usd`), `ClearScene`,
/// `RestartScene` — needs exactly this set. Bundling keeps the mount API honest:
/// a caller drives a teardown without naming `WorldGrid`, `OriginAnchor` or the
/// cosim `SimConnection` wire type, so `lunco-usd` needs no dependency on
/// `lunco-cosim` to orchestrate a scene swap.
#[derive(bevy::ecs::system::SystemParam)]
pub struct SceneEntities<'w, 's> {
    grid: Query<'w, 's, &'static Children, With<WorldGrid>>,
    origin: Query<'w, 's, Entity, With<OriginAnchor>>,
    /// Every active scene root identifies the USD stage whose generated prims
    /// belong to that scene.  A camera mount is allowed to move a prim directly
    /// under the persistent grid, so hierarchy alone is not sufficient to find
    /// all of these on teardown.
    scene_roots: Query<'w, 's, &'static UsdPrimPath, With<UsdSceneRoot>>,
    prims: Query<'w, 's, (Entity, &'static UsdPrimPath)>,
    wires: Query<'w, 's, Entity, With<SimConnection>>,
    /// The sandbox's code-spawned safety camera is deliberately outside the
    /// USD subtree, so it needs explicit scene-lifecycle ownership.  Leaving it
    /// alive while a replacement USD Avatar claims the viewport is the only
    /// route to two local cameras after a full reload.
    provisional_avatars: Query<'w, 's, Entity, With<lunco_avatar::ProvisionalAvatarCamera>>,
}

pub fn clear_scene_entities(commands: &mut Commands, scene: &SceneEntities) {
    // A scene is its entities AND the resources derived from it. Resources are
    // restored through the registry rather than named here, so a subsystem that
    // adds scene-derived state does not also have to edit this function — see
    // `lunco_usd_bevy::scene_lifecycle`.
    commands.queue(lunco_usd_bevy::scene_lifecycle::run_scene_teardown);

    let (q_grid, q_origin, q_scene_roots, q_prims, q_wires, q_provisional_avatars) = (
        &scene.grid,
        &scene.origin,
        &scene.scene_roots,
        &scene.prims,
        &scene.wires,
        &scene.provisional_avatars,
    );
    let mut despawned = 0usize;

    // Despawn all children of the WorldGrid (recursively), except the persistent OriginAnchor
    if let Ok(children) = q_grid.single() {
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
        if active_stage_ids.contains(&prim.stage_handle.id()) {
            commands.entity(entity).try_despawn();
            stage_prim_despawns += 1;
        }
    }

    // The despawn above takes the scene camera with it, and that camera is usually
    // what holds `FloatingOrigin` (`process_usd_sim_prims` strips it off the anchor
    // when a USD Avatar prim claims it). Hand it back to the anchor in THIS flush.
    // Leaving the gap for `anchor_owns_origin_by_default` to close in PostUpdate is
    // what logged "BigSpace … has no floating origins" on every scene change: the
    // guard is a backstop, not the handover. `try_insert` is a no-op if the anchor
    // already holds it (the origin never left home for this scene).
    if let Ok(anchor) = q_origin.single() {
        commands
            .entity(anchor)
            .try_insert(big_space::prelude::FloatingOrigin);
    }

    // Despawn any root-level derived connection wires (which are spawned as root entities)
    for e in q_wires.iter() {
        commands.entity(e).try_despawn();
        despawned += 1;
    }

    // A provisional avatar belongs to the loaded scene, even though it is
    // spawned by the UI rather than from a USD prim.  Remove it in the same
    // deferred batch as the old stage so it cannot render alongside the next
    // scene's authored avatar during a reload.
    for e in q_provisional_avatars.iter() {
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
/// `stage_handle_id` scopes the lookup to one scene; `reader` is the *fresh*
/// composed stage (the asset store's current reader, so the `on_usd_prim_added`
/// observer that fires on the new `UsdPrimPath` sees the prim). The parent live
/// entity is found by composed path; the child is spawned with the same atomic
/// `(UsdPrimPath, ChildOf, transform, instance-membership)` bundle the loader
/// uses, so the observer instantiates its geometry + subtree in place without
/// disturbing siblings. Returns `None` (no-op) if the parent isn't live yet or
/// the prim is already spawned.
pub fn spawn_usd_child(
    world: &mut World,
    stage_handle_id: bevy::asset::AssetId<UsdStageAsset>,
    reader: &lunco_usd_bevy::StageView<'_>,
    path: &str,
) -> Option<Entity> {
    // Pre-populate the translate so physics sees the spawn offset before the
    // observer refines the full transform (matches the loader's child branch).
    let sdf_path = SdfPath::new(path).ok()?;
    let tf = lunco_usd_bevy::get_attribute_as_vec3(reader, &sdf_path, "xformOp:translate")
        .map(Transform::from_translation)
        .unwrap_or_default();
    spawn_usd_child_with_translate(world, stage_handle_id, path, tf)
}

/// Reader-free core of [`spawn_usd_child`]: spawn the stub child entity for
/// `path` under its already-live parent, with a pre-read transform `tf`,
/// inheriting grid-anchoring + instance membership from the parent. The
/// `on_usd_prim_added` observer then builds the subtree from the canonical
/// stage.
///
/// Split out so the live-stage projection bridge can pre-read the translate
/// under a *short* immutable borrow of the `!Send` `CanonicalStage` and then
/// spawn here with `&mut World` — the stage itself can't be held across the
/// spawn (it aliases the world), but the observer that fires on insert reads it
/// fresh from `CanonicalStages`.
pub fn spawn_usd_child_with_translate(
    world: &mut World,
    stage_handle_id: bevy::asset::AssetId<UsdStageAsset>,
    path: &str,
    tf: Transform,
) -> Option<Entity> {
    // Parent path = `path` minus its final `/segment`.
    let (parent_prefix, _name) = path.rsplit_once('/')?;
    let parent_path = if parent_prefix.is_empty() {
        "/"
    } else {
        parent_prefix
    };

    // Resolve the live parent entity (same scene) and bail if it isn't
    // instantiated yet — a following full load / reconcile will cover it.
    let parent_entity = {
        let mut q = world.query::<(Entity, &UsdPrimPath)>();
        q.iter(world)
            .find(|(_, upp)| upp.stage_handle.id() == stage_handle_id && upp.path == parent_path)
            .map(|(e, _)| e)
    }?;
    // Idempotent: never double-spawn a path that already has a live entity.
    let already = {
        let mut q = world.query::<&UsdPrimPath>();
        q.iter(world)
            .any(|upp| upp.stage_handle.id() == stage_handle_id && upp.path == path)
    };
    if already {
        return None;
    }

    let stage_handle = world
        .get::<UsdPrimPath>(parent_entity)?
        .stage_handle
        .clone();

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
    // Plain child of its USD parent, per the anchoring contract: the scene root
    // is the one grid anchor and everything under it inherits that frame. A prim
    // carrying its own `CellCoord` under the grid fights avian's writeback and
    // freezes its render (see `instantiate_usd_prim` / `SpawnAnchor`).
    let entity = match member {
        Some(m) => world.spawn((base, ChildOf(parent_entity), m)).id(),
        None => world.spawn((base, ChildOf(parent_entity))).id(),
    };
    info!("[scene] incremental spawn: `{}` (entity {})", path, entity);
    Some(entity)
}

/// Normalize a scene path to asset-server-relative form. Accepts an
/// absolute path under the workspace `assets/` dir (Twin manifests store
/// scenes as twin-root-relative; the caller joins them to an absolute
/// path) or an already-relative asset path. Returns `None` (with a warn)
/// if an absolute path lies outside the assets dir or a relative path repeats
/// the asset-root `assets/` prefix.
pub fn normalize_scene_asset_path(path_in: &str) -> Option<String> {
    // Already a scheme path (`abs://`, `lunco://`, …) — the AssetServer routes
    // it to the named source as-is.
    if lunco_assets::has_scheme(path_in) {
        return Some(path_in.to_string());
    }
    let pb = std::path::PathBuf::from(path_in);
    if pb.is_absolute() {
        // Under the project `assets/` dir → asset-relative (default source).
        // `lunco-assets` owns that mapping; this only decides what `LoadScene`
        // does when it does NOT apply.
        match lunco_assets::library_rel(&pb) {
            Some(rel) => Some(rel),
            None => {
                // `LoadScene` takes SCHEME-QUALIFIED addresses (`lunco://`,
                // `twin://`) — it loads an already-addressable asset and has no
                // access to the workspace/Twin layer, so it cannot resolve a
                // bare filesystem path to a root or mount it doc-first.
                //
                // `OpenFile` is the entry point that owns that step: it resolves
                // the scene's root, registers it, and mounts through the document
                // overlay. Routing a raw path here instead would mount a
                // base-only stage and silently drop runtime edits.
                warn!(
                    "[scene] `{}` is a bare filesystem path — `LoadScene` takes \
                     scheme addresses (`lunco://…`, `twin://…`). Use `OpenFile` \
                     to open a scene by path; it resolves the owning root.",
                    path_in
                );
                None
            }
        }
    } else {
        // `LoadScene` is asset-root-relative. Passing `assets/scenes/...`
        // would make AssetServer resolve `assets/assets/scenes/...`; if the
        // caller cleared the active scene first, that typo left the viewport
        // scene-less and therefore unlit. Reject it before any scene lifecycle
        // work instead of accepting a second spelling for the same asset.
        let normalized = path_in.replace('\\', "/");
        if normalized == "assets" || normalized.starts_with("assets/") {
            warn!(
                "[scene] `{}` repeats the asset root; LoadScene paths are relative to `assets/` \
                 (use `scenes/...`, not `assets/scenes/...`)",
                path_in
            );
            None
        } else {
            Some(path_in.to_string())
        }
    }
}

/// Spawn a USD scene root under the first `Grid` entity.
///
/// Shared by `LoadScene` (after its clear step) and `OpenFile` (additive
/// import). Blender-style no-op when the same `(asset, root_prim)` is
/// already mounted. Returns the spawned entity, or `None` on no-op /
/// missing `Grid`.
pub fn spawn_scene_root_world(
    world: &mut World,
    path_in: &str,
    root_prim_in: &str,
) -> Option<Entity> {
    // Normalize to asset-server-relative. The asset server prepends
    // its configured `file_path` (the `assets/` root) to every load
    // string, so absolute paths must have that prefix stripped.
    let Some(asset_path) = normalize_scene_asset_path(path_in) else {
        return None;
    };
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
#[derive(Component, Debug, Clone, Copy)]
pub struct UsdSceneRoot;

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
/// (see [`resolve_root_prim`]). Blender-style no-op when the same
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
    // it builds the persistent shell (root + WorldGrid + single FloatingOrigin) on
    // the first scene load and returns the same grid on every reload — so the root
    // is never duplicated and never absent. Replaces the old "first `Grid` found"
    // heuristic, which was ambiguous once celestial / preview grids also existed.
    let grid = lunco_core::ensure_world_root(world);

    // Scene-root entity is itself the Grid-direct `GridAnchor`. Its
    // children — top-level USD prims (rovers, balls, terrain) — stay
    // as plain Bevy children, inheriting GlobalTransform from this
    // anchor via Bevy's normal transform propagation (handled by
    // big_space's `propagate_low_precision`). This restores the working
    // hierarchy where avian rigid bodies on rover roots compute
    // `Position` relative to the scene-root anchor instead of needing
    // their own CellCoord, which conflicted with avian's writeback.
    // Atomic spawn: `ChildOf(grid)` in the bundle so parent + CellCoord +
    // Transform land together — same contract as `migrate_to_grid`. Avoids
    // the observer race that mis-tagged rover chassis as `RigidBody::Static`.
    let root = world
        .spawn((
            Name::new(format!("Scene:{}", asset_path)),
            UsdSceneRoot,
            UsdPrimPath {
                stage_handle: handle,
                path: root_prim.clone(),
            },
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
///    (falling back to `/` — whole-stage mount — when none is declared).
///
/// The defaultPrim lookup is deliberately deferred rather than read
/// here: this runs synchronously at command time, before the stage
/// asset finishes loading. It is resolved from the parsed `TextReader`
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

    // Ensure the source asset types this module's systems read/allocate are
    // registered. Idempotent — production registers these via the Modelica /
    // scripting plugins; doing it here lets minimal apps (headless tests using
    // `MinimalPlugins` without those plugins) run the cosim systems without
    // panicking on a missing `Assets<…>` resource.
    app.init_asset::<ModelicaSource>()
        .init_asset::<PythonSource>()
        .init_resource::<lunco_scripting::ScriptRegistry>()
        .init_resource::<WiringDirty>()
        .init_resource::<BindingEpochDirty>()
        .init_resource::<BindingModelStatuses>()
        .init_resource::<crate::domain_projection::MemberClasses>()
        .init_resource::<crate::domain_projection::ProjectionDirty>()
        // The open synthesizer registry (doc 37 §8): the acausal-network
        // synthesizer registers itself as the default; another domain is a
        // `register()` from any plugin, not an edit here.
        .init_resource::<crate::domain_projection::SynthesizerRegistry>();
    app.add_observer(request_binding_epoch::<UsdPrimPath>)
        .add_observer(request_binding_epoch_on_remove::<UsdPrimPath>)
        .add_observer(request_binding_epoch::<ModelicaModel>)
        .add_observer(request_binding_epoch_on_remove::<ModelicaModel>)
        .add_observer(request_binding_epoch::<SimComponent>)
        .add_observer(forget_binding_model_status)
        .add_observer(request_binding_epoch::<lunco_usd_avian::PendingUsdJoint>)
        .add_observer(request_binding_epoch_on_remove::<lunco_usd_avian::PendingUsdJoint>)
        .add_observer(request_binding_epoch::<crate::PendingWheelWiring>)
        .add_observer(request_binding_epoch_on_remove::<crate::PendingWheelWiring>)
        .add_observer(request_binding_epoch::<crate::PendingDifferential>)
        .add_observer(request_binding_epoch_on_remove::<crate::PendingDifferential>)
        .add_observer(request_binding_epoch::<SimConnection>)
        .add_observer(request_binding_epoch_on_remove::<SimConnection>);
    // USD source-load and contract failures use the same core notice stream as
    // the Modelica compiler, so the workbench console has one observable error
    // surface. `add_message` is idempotent when the Modelica plugin registered
    // it already.
    app.add_message::<lunco_modelica::ModelicaNotice>();

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
            .after(lunco_usd_bevy::sync_usd_visuals),
    );

    app.add_systems(
        Update,
        // Drain the single-flight guard the frame after the last prim of
        // the in-flight scene leaves the awaiting pool. Cheap (one
        // `Option<Res>` + a bounded `Query::iter` only when a guard is
        // set); no per-frame cost in steady state.
        clear_scene_load_in_flight.after(lunco_usd_bevy::sync_usd_visuals),
    );

    app.add_systems(
        Update,
        settle_binding_epoch
            .after(CosimUpdateSet::Projection)
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
            project_usd_telemetry,
            // Source-load drain runs every Update; cheap when no
            // `PendingModelicaSource` / `PendingPythonSource` entities
            // exist. Splitting it from `process_usd_cosim_prims` is
            // intentional — the source asset may take multiple frames
            // to load (network on wasm, async I/O on native).
            dispatch_loaded_modelica_sources,
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
        (
            crate::domain_projection::project_domain_islands,
            crate::domain_projection::sync_generated_network_documents,
            crate::domain_projection::publish_generated_sources,
        )
            .chain()
            .in_set(CosimUpdateSet::Projection),
    );

    app.add_systems(
        Update,
        (
            // Wiring is derived from native `connectionPaths`: rebuilds the
            // `SimConnection` set whenever prims spawn/despawn (structural) or a
            // `connectionPaths` edit is drained (`WiringDirty`); dormant otherwise.
            rewire_usd_connections,
            wrap_modelica_into_simcomponent.run_if(any_unwrapped_modelica),
            // Parameters: the authored constants the wiring pass gathered off the
            // unconnected `inputs:` ports, pushed into the model once it exists.
            // After the wrap, because it needs the `SimComponent` to write into.
            seed_usd_input_defaults,
            // §6 opaque guard: once a body is cosim-driven, mark it unpredictable
            // (after the SimComponent wrap above, so it sees freshly-wrapped bodies).
            tag_cosim_opaque,
        )
            .chain()
            .in_set(CosimUpdateSet::Wiring),
    );

    app.add_systems(
        FixedUpdate,
        (
            validate_usd_modelica_port_contracts.before(sync_modelica_outputs),
            sync_modelica_outputs.before(PropagateCosimSet::Propagate),
            sync_script_outputs.before(PropagateCosimSet::Propagate),
            sync_modelica_inputs
                .after(ApplyForcesCosimSet::ApplyForces)
                .before(ModelicaSet::SpawnRequests),
            sync_script_inputs
                .after(ApplyForcesCosimSet::ApplyForces)
                .before(ModelicaSet::SpawnRequests),
            // Modelica `when` bridge: edge-detect on fresh outputs, after they sync.
            fire_connected_events
                .after(sync_modelica_outputs)
                .after(sync_script_outputs),
        ),
    );

    app.add_systems(
        Startup,
        |reg: Option<ResMut<lunco_api::ApiQueryRegistry>>| {
            if let Some(mut reg) = reg {
                // Canonical uniform port verbs (over `lunco_cosim::ports`).
                reg.register(ListPortsProvider);
                reg.register(GetPortProvider);
                reg.register(SetPortProvider);
                reg.register(ReleasePortProvider);
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
    fn scene_path_repeating_asset_root_is_rejected_before_reload() {
        assert_eq!(
            normalize_scene_asset_path("assets/scenes/luncosim/sandbox_scene.usda"),
            None
        );
        assert_eq!(
            normalize_scene_asset_path("assets\\scenes\\sandbox\\sandbox_scene.usda"),
            None
        );
        assert_eq!(
            normalize_scene_asset_path("scenes/luncosim/sandbox_scene.usda"),
            Some("scenes/luncosim/sandbox_scene.usda".to_string())
        );
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

    /// A model that has been parsed and dispatched but not yet solved:
    /// declared inputs, no variables.
    fn dispatched_but_unsolved() -> ModelicaModel {
        let mut m = ModelicaModel {
            model_name: "GeneratedElectrical".into(),
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
    fn status_tracks_compile_run_pause_and_failure() {
        let mut model = dispatched_but_unsolved();
        assert_eq!(modelica_status(&model), SimStatus::Compiling);
        model.variables.insert("soc_out".into(), 1.0);
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

        let ready = SimComponent {
            status: SimStatus::Idle,
            ..default()
        };
        assert!(modelica_models_terminal(std::iter::once((
            Some(&model),
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

        copy_modelica_input_values(&mut model, &component);

        assert_eq!(model.inputs.get("guidance_throttle"), Some(&0.75));
        assert!(
            !model.inputs.contains_key("force_y"),
            "a physical sink must remain outside the Modelica input map"
        );
    }

    #[test]
    fn connected_event_fires_once_per_rising_edge_and_rearms() {
        let mut armed = true;
        assert!(!event_rising_edge(&mut armed, 0.0));
        assert!(event_rising_edge(&mut armed, 0.5));
        assert!(!event_rising_edge(&mut armed, 1.0));
        assert!(!event_rising_edge(&mut armed, 0.49));
        assert!(event_rising_edge(&mut armed, 1.0));
    }

    #[test]
    fn event_severity_defaults_to_info() {
        assert_eq!(
            parse_event_severity("not-a-severity"),
            lunco_core::Severity::Info
        );
        assert_eq!(
            parse_event_severity("critical"),
            lunco_core::Severity::Critical
        );
    }
}
