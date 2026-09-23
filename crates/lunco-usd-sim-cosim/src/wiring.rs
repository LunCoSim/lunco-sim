use super::*;
use lunco_usd_bevy_scene::UsdSceneAwaitingStage;
use std::collections::HashSet;

/// (as opposed to authored some other way). [`rewire_usd_connections`] reconciles
/// tagged edges against composed USD wiring, preserving unchanged runtime edges.
#[derive(Component, Default)]
pub struct UsdWiredConnection;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct WiringFactsKey {
    stage: bevy::asset::AssetId<UsdStageAsset>,
    generation: u64,
    instance: Option<u64>,
}

#[derive(Resource, Default)]
pub(super) struct WiringFactsCache(std::collections::HashMap<WiringFactsKey, StageWiringFacts>);

#[derive(Default)]
struct StageWiringFacts {
    modelica_members: Option<std::collections::HashSet<String>>,
    prims: std::collections::HashMap<String, PrimWiringFacts>,
}

struct PrimWiringFacts {
    type_name: Option<String>,
    is_domain_root: bool,
    attributes: Vec<AttributeWiringFacts>,
}

struct AttributeWiringFacts {
    name: String,
    connections: Vec<String>,
    value: Option<f64>,
    scale: f64,
    offset: f64,
    is_network_boundary_output: bool,
    feeds_internal_network_input: bool,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct ConnectionIdentity {
    start_element: Entity,
    start_connector: String,
    start_is_input: bool,
    end_element: Entity,
    end_connector: String,
}

struct PreviousWiredEdge {
    entity: Entity,
    connection: SimConnection,
    name: Option<String>,
}

fn connection_identity(connection: &SimConnection) -> ConnectionIdentity {
    ConnectionIdentity {
        start_element: connection.start_element,
        start_connector: connection.start_connector.clone(),
        start_is_input: connection.start_is_input,
        end_element: connection.end_element,
        end_connector: connection.end_connector.clone(),
    }
}

fn same_connection(left: &SimConnection, right: &SimConnection) -> bool {
    left.start_element == right.start_element
        && left.start_connector == right.start_connector
        && left.start_is_input == right.start_is_input
        && left.end_element == right.end_element
        && left.end_connector == right.end_connector
        && left.scale.to_bits() == right.scale.to_bits()
        && left.offset.to_bits() == right.offset.to_bits()
}

fn read_prim_wiring_facts(view: &dyn UsdReadObject, prim: &SdfPath) -> PrimWiringFacts {
    let type_name = view.type_name(prim);
    let is_domain_root = lunco_usd_sim_domain::is_runtime_domain_network_root(view, prim);
    let attributes = view
        .attr_names(prim)
        .into_iter()
        .filter(|name| name.starts_with("inputs:") || name.starts_with("outputs:"))
        .map(|name| {
            let sink_conn = name
                .strip_prefix("inputs:")
                .or_else(|| name.strip_prefix("outputs:"))
                .unwrap_or(&name);
            let sink_conn = sink_conn.strip_suffix(".connect").unwrap_or(sink_conn);
            let is_output = name.starts_with("outputs:");
            AttributeWiringFacts {
                connections: view.connections(prim, &name),
                value: view.real(prim, &name),
                scale: view
                    .real(prim, &format!("lunco:factor:{sink_conn}"))
                    .unwrap_or(1.0),
                offset: view
                    .real(prim, &format!("lunco:offset:{sink_conn}"))
                    .unwrap_or(0.0),
                is_network_boundary_output: is_domain_root
                    && is_output
                    && lunco_usd_bevy_core::program::is_network_boundary_output(view, prim, &name),
                feeds_internal_network_input: is_domain_root
                    && !is_output
                    && lunco_usd_bevy_core::program::internal_network_input_source(
                        view, prim, sink_conn,
                    )
                    .is_some(),
                name,
            }
        })
        .collect();
    PrimWiringFacts {
        type_name,
        is_domain_root,
        attributes,
    }
}

/// Queries used by the wiring projection. Keeping them in one system parameter
/// leaves the projection below the Bevy system-parameter arity limit while
/// keeping each query's ownership and change-detection semantics explicit.
#[derive(SystemParam)]
pub(super) struct WiringQueries<'w, 's> {
    endpoints: Query<
        'w,
        's,
        (
            Entity,
            &'static UsdPrimPath,
            Has<ModelicaModel>,
            Option<&'static GeneratedModelicaSource>,
            Has<lunco_environment::EnvironmentProbe>,
            Option<&'static lunco_port_core::PortSurface>,
            Option<&'static UsdInstanceProjection>,
        ),
        Or<(
            With<lunco_port_core::PortSurfaceReady>,
            With<lunco_port_core::PortSurface>,
            With<lunco_port_core::OutputPorts>,
            With<SimComponent>,
        )>,
    >,
    edges: Query<
        'w,
        's,
        (
            Entity,
            Option<&'static SimConnection>,
            Option<&'static Name>,
        ),
        With<UsdWiredConnection>,
    >,
    global_ids: Query<'w, 's, &'static lunco_core::GlobalEntityId>,
    provenance: Query<'w, 's, &'static lunco_core::Provenance>,
    instance_roots: Query<'w, 's, (), With<UsdInstanceRoot>>,
    realtime_safe: Query<'w, 's, &'static lunco_cosim_core::RealtimeSafe>,
    predicted_bodies: Query<
        'w,
        's,
        &'static avian3d::prelude::RigidBody,
        Without<lunco_core_session::NotPredictable>,
    >,
    defaults: Query<'w, 's, &'static UsdInputDefaults>,
    outputs: Query<'w, 's, &'static lunco_port_core::OutputPorts>,
}

/// Run condition for the derived USD wiring cache.
///
/// Endpoint lifecycle observers, the live-stage consumer, and network-role
/// changes publish the dirty latch. Reading it avoids scanning the endpoint
/// population on every stable update just to rediscover that no `Added<T>`
/// filter matches.
pub(super) fn wiring_due(
    dirty: Res<UsdWiringDirty>,
    role: Option<Res<lunco_core_session::NetworkRole>>,
) -> bool {
    dirty.0 || role.is_some_and(|role| role.is_changed())
}

pub(super) fn mark_wiring_dirty_on_remove<T: Component>(
    _trigger: On<Remove, T>,
    mut dirty: ResMut<UsdWiringDirty>,
) {
    dirty.0 = true;
}

fn mark_wiring_dirty_for_endpoint_add<T: Component>(
    trigger: On<Add, T>,
    endpoints: Query<
        (),
        (
            With<UsdPrimPath>,
            Or<(
                With<lunco_port_core::PortSurfaceReady>,
                With<lunco_port_core::PortSurface>,
                With<lunco_port_core::OutputPorts>,
                With<SimComponent>,
            )>,
        ),
    >,
    mut dirty: ResMut<UsdWiringDirty>,
) {
    if endpoints.contains(trigger.entity) {
        dirty.0 = true;
    }
}

fn mark_wiring_dirty_for_usd_endpoint_remove<T: Component>(
    trigger: On<Remove, T>,
    endpoints: Query<
        (),
        (
            With<UsdPrimPath>,
            Or<(
                With<lunco_port_core::PortSurfaceReady>,
                With<lunco_port_core::PortSurface>,
                With<lunco_port_core::OutputPorts>,
                With<SimComponent>,
            )>,
        ),
    >,
    mut dirty: ResMut<UsdWiringDirty>,
) {
    if endpoints.contains(trigger.entity) {
        dirty.0 = true;
    }
}

fn mark_wiring_dirty_for_path_remove(
    trigger: On<Remove, UsdPrimPath>,
    endpoints: Query<
        (),
        Or<(
            With<lunco_port_core::PortSurfaceReady>,
            With<lunco_port_core::PortSurface>,
            With<lunco_port_core::OutputPorts>,
            With<SimComponent>,
        )>,
    >,
    mut dirty: ResMut<UsdWiringDirty>,
) {
    if endpoints.contains(trigger.entity) {
        dirty.0 = true;
    }
}

pub(super) fn install_wiring_invalidation_observers(app: &mut App) {
    app.add_observer(mark_wiring_dirty_for_endpoint_add::<lunco_port_core::PortSurfaceReady>)
        .add_observer(mark_wiring_dirty_for_endpoint_add::<lunco_port_core::PortSurface>)
        .add_observer(mark_wiring_dirty_for_endpoint_add::<lunco_port_core::OutputPorts>)
        .add_observer(mark_wiring_dirty_for_endpoint_add::<SimComponent>)
        .add_observer(mark_wiring_dirty_for_endpoint_add::<UsdPrimPath>)
        .add_observer(mark_wiring_dirty_for_endpoint_add::<lunco_core::GlobalEntityId>)
        .add_observer(mark_wiring_dirty_for_endpoint_add::<UsdInstanceProjection>)
        .add_observer(mark_wiring_dirty_on_remove::<lunco_port_core::PortSurfaceReady>)
        .add_observer(mark_wiring_dirty_on_remove::<lunco_port_core::PortSurface>)
        .add_observer(mark_wiring_dirty_on_remove::<lunco_port_core::OutputPorts>)
        .add_observer(mark_wiring_dirty_on_remove::<SimComponent>)
        .add_observer(mark_wiring_dirty_for_path_remove)
        .add_observer(mark_wiring_dirty_for_usd_endpoint_remove::<lunco_core::GlobalEntityId>)
        .add_observer(mark_wiring_dirty_for_usd_endpoint_remove::<UsdInstanceProjection>);
}

/// Last published Modelica participant status. `SimComponent` also carries
/// continuously changing inputs/outputs, so Bevy's broad `Changed<SimComponent>`
/// signal is not by itself a binding-lifecycle event.
#[derive(Resource, Default)]
pub(super) struct BindingModelStatuses(HashMap<Entity, SimStatus>);

#[derive(Resource)]
pub struct BindingEpochWait(pub(crate) lunco_readiness::ReadinessTicket);

pub(super) fn request_binding_epoch<T: Component>(
    _trigger: On<Add, T>,
    mut dirty: ResMut<BindingEpochDirty>,
) {
    dirty.0 = true;
}

pub(super) fn request_binding_epoch_on_remove<T: Component>(
    _trigger: On<Remove, T>,
    mut dirty: ResMut<BindingEpochDirty>,
) {
    dirty.0 = true;
}

pub(super) fn request_binding_epoch_on_model_change(
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

pub(super) fn forget_binding_model_status(
    trigger: On<Remove, SimComponent>,
    mut statuses: ResMut<BindingModelStatuses>,
    mut dirty: ResMut<BindingEpochDirty>,
) {
    statuses.0.remove(&trigger.entity);
    dirty.0 = true;
}

pub fn modelica_models_terminal<'a>(
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
/// hold. `PendingUsdJoint` and `PendingDifferential` are
/// local activation gates: the affected bodies remain kinematic until their
/// endpoint is ready. Their preparation runs in the fixed schedule and their
/// structural admission runs in the outer `Update` schedule, so globally
/// pausing Avian's nested `PhysicsSchedule` does not deadlock those markers.
/// A deferred USD stage is different: its projection can still add arbitrary
/// bodies and connections, so it retains the world hold until the stage is
/// available.
pub(super) fn settle_binding_epoch(
    awaiting: Query<(), With<UsdSceneAwaitingStage>>,
    joints: Query<(), With<lunco_usd_avian_contracts::PendingUsdJoint>>,
    differentials: Query<(), With<PendingDifferential>>,
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
    mut revision: ResMut<lunco_cosim_core::BindingRevision>,
    wait: Option<Res<BindingEpochWait>>,
    mut readiness: ResMut<lunco_readiness::ReadinessRegistry>,
    mut commands: Commands,
) {
    let models_terminal = modelica_models_terminal(models.iter());
    let settled =
        awaiting.is_empty() && joints.is_empty() && differentials.is_empty() && models_terminal;
    if settled {
        dirty.0 = false;
        revision.seal_epoch();
    } else {
        // Keep the reconciliation scheduled until every deferred stage,
        // joint, differential, and model participant has reached a
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
/// are a **pure derived cache**: whenever the wiring topology may have changed,
/// desired edges are recomputed from composed USD and reconciled with existing
/// edges. Unchanged edges retain their identity and binding state.
///
/// Trigger (dormant otherwise — steady state is zero work):
/// - **structural** — a simulation endpoint or its port surface is added or
///   removed. Covers initial scene load, async payload/vessel spawn,
///   source-after-sink ordering, and a generated island publishing its boundary
///   contract; visual-only prims are not wiring endpoints.
/// - **live edit** — [`UsdWiringDirty`], set by the op-driven USD command
///   runtime when a `connectionPaths` change is drained from the live stage
///   (an edit that is not itself a prim spawn/despawn).
///
/// A connection whose source prim is not yet spawned is skipped (its later spawn
/// re-runs this); a malformed source path is logged and skipped — restoring the
/// diagnostic the deleted `process_usd_cosim_wire_read` emitted.
/// Cache entries are scoped to a composed stage generation and instance.
///
/// `UsdSimCosimPlugin` installs this system as part of its normal update pipeline.
/// This narrow installer is also useful to headless integration hosts that
/// provide the wiring resources and want to exercise this owner without
/// assembling the complete application plugin graph.
pub fn install_wiring_system(app: &mut App) {
    app.init_resource::<UsdWiringDirty>();
    app.init_resource::<WiringFactsCache>();
    app.world_mut().resource_mut::<UsdWiringDirty>().0 = true;
    install_wiring_invalidation_observers(app);
    app.add_systems(Update, rewire_usd_connections);
}

pub(super) fn reset_wiring_facts_cache(mut cache: ResMut<WiringFactsCache>) {
    cache.0.clear();
}

pub(super) fn rewire_usd_connections(
    mut commands: Commands,
    mut dirty: ResMut<UsdWiringDirty>,
    mut facts_cache: ResMut<WiringFactsCache>,
    // Wiring consumes a projected endpoint, not an initial path stub. The
    // grouped query parameter keeps this system within Bevy's arity limit;
    // the endpoint marker remains the authoritative admission contract.
    wiring: WiringQueries,
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
    // The realtime gate: whether the SOURCE program promised it is realtime-safe,
    // and whether the SINK is a client-predicted dynamic body (a `RigidBody` NOT
    // opted out of prediction). The network role is part of this contract: only
    // a pure client predicts the body locally. Standalone and host processes are
    // authoritative, so their live solver is not incorrectly classified as a
    // prediction loop.
    role: Option<Res<lunco_core_session::NetworkRole>>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
) {
    let client_predicts = matches!(
        role.as_deref(),
        Some(lunco_core_session::NetworkRole::Client)
    );
    let role_changed = role.as_ref().is_some_and(|role| role.is_changed());

    // Changing authority changes whether a force edge is admissible. Rebuild
    // immediately on a standalone/host ↔ client transition instead of leaving
    // the previous role's wiring decision cached.
    if !role_changed && !dirty.0 {
        return;
    }
    dirty.0 = false;

    // A prim's instance identity (its instance-root GID, `None` for scene prims)
    // is what keeps two spawns of one asset — byte-identical stage-relative paths
    // and all — from collapsing onto one entity below. See `instance_key`.
    let instance_of = |e: Entity, projection: Option<&UsdInstanceProjection>| {
        lunco_usd_bevy_scene::instance_key_from_projection(
            e,
            &wiring.provenance,
            &wiring.global_ids,
            &wiring.instance_roots,
            projection,
        )
    };
    let mut active_cache_keys = HashSet::new();

    // Index every prim entity by (stage, instance, path). The stage is part of
    // prim identity: two composed USD projections may carry the same path text
    // while belonging to different stage assets. Omitting it lets a later
    // projection silently overwrite the first one, which can bind a simulation
    // wire to a transform-only entity instead of the light/material/physics
    // projection that owns the named port.
    //
    // The instance key still keeps two runtime spawns of one stage distinct;
    // the stage key keeps independently composed stages distinct.
    let mut by_path: HashMap<(bevy::asset::AssetId<UsdStageAsset>, Option<u64>, &str), Entity> =
        HashMap::new();
    // A generated network is one Modelica participant, while its composed
    // member paths remain valid USD addresses for presentation and external
    // scalar consumers. This table translates those member output addresses to
    // the generated wrapper output declared by the projection. It is derived
    // from generated source metadata, not from any vehicle, sensor, or renderer
    // type, so every generated domain gets the same boundary behavior.
    let mut generated_member_outputs: HashMap<
        (bevy::asset::AssetId<UsdStageAsset>, Option<u64>, &str, &str),
        (Entity, &str),
    > = HashMap::new();
    let environment_probe_entities: std::collections::HashSet<Entity> = wiring
        .endpoints
        .iter()
        .filter_map(|(entity, _, _, _, is_probe, _, _)| is_probe.then_some(entity))
        .collect();
    let port_surfaces: HashMap<Entity, &lunco_port_core::PortSurface> = wiring
        .endpoints
        .iter()
        .filter_map(|(entity, _, _, _, _, surface, _)| surface.map(|surface| (entity, surface)))
        .collect();
    for (e, p, _, generated, _, _, projection) in wiring.endpoints.iter() {
        let instance = instance_of(e, projection);
        let key = (p.stage_handle.id(), instance, p.path.as_str());
        by_path.insert(key, e);
        if let Some(generated) = generated {
            for (member, output, alias) in &generated.member_output_aliases {
                generated_member_outputs.insert(
                    (
                        p.stage_handle.id(),
                        instance,
                        member.as_str(),
                        output.as_str(),
                    ),
                    (e, alias.as_str()),
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

    // Reuse identical edge entities so a new endpoint does not invalidate and
    // rebind every connection in the scene. A changed authored edge is replaced
    // through the normal add/remove lifecycle.
    let mut previous_edges: HashMap<ConnectionIdentity, std::collections::VecDeque<_>> =
        HashMap::new();
    for (entity, connection, name) in wiring.edges.iter() {
        let Some(connection) = connection else {
            commands.entity(entity).try_despawn();
            continue;
        };
        previous_edges
            .entry(connection_identity(connection))
            .or_default()
            .push_back(PreviousWiredEdge {
                entity,
                connection: connection.clone(),
                name: name.map(|name| name.as_str().to_string()),
            });
    }

    for (entity, prim_path, has_modelica, _, _, wheel_endpoints, projection) in
        wiring.endpoints.iter()
    {
        let id = prim_path.stage_handle.id();
        let Some(stage_asset) = stages.get(&prim_path.stage_handle) else {
            continue;
        };
        let sink_instance = instance_of(entity, projection);
        let (reader, generation) = canonical.reader_for_entity(id, stage_asset, projection);
        let view: &dyn UsdReadObject = &reader;
        let Ok(sink_sdf) = SdfPath::new(&prim_path.path) else {
            continue;
        };
        let cache_key = WiringFactsKey {
            stage: id,
            generation,
            instance: sink_instance,
        };
        active_cache_keys.insert(cache_key);
        let stage_facts = facts_cache.0.entry(cache_key).or_default();
        if stage_facts.modelica_members.is_none() {
            stage_facts.modelica_members = Some(
                lunco_usd_bevy_core::program::modelica_network_member_paths(view),
            );
        }
        if !stage_facts.prims.contains_key(&prim_path.path) {
            stage_facts.prims.insert(
                prim_path.path.clone(),
                read_prim_wiring_facts(view, &sink_sdf),
            );
        }
        let is_modelica_member = stage_facts
            .modelica_members
            .as_ref()
            .is_some_and(|members| members.contains(&prim_path.path));
        let Some(prim_facts) = stage_facts.prims.get(&prim_path.path) else {
            continue;
        };
        // `LunCoEvent.inputs:trigger` is a standard USD connection, but its
        // consumer is the event projector below rather than the scalar
        // SimConnection fabric.
        if prim_facts.type_name.as_deref() == Some("LunCoEvent") {
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
        if is_modelica_member {
            continue;
        }

        // Resolve this prim's wires within its OWN instance — a source path names a
        // prim of the same spawn, never a same-named prim of a different one.
        for attribute in &prim_facts.attributes {
            let attr = attribute.name.as_str();
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
                prim_facts.type_name.as_deref(),
                Some("Material" | "Shader" | "NodeGraph")
            );
            let forward = attr
                .strip_prefix("outputs:")
                .filter(|_| !shading_prim)
                .map(|name| {
                    match wiring
                        .outputs
                        .get(entity)
                        .ok()
                        .and_then(|outputs| outputs.get(name))
                    {
                        Some(port_entity) => (port_entity, lunco_cosim_core::PORT_NAME.to_string()),
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
            // Same reasoning, one level up — but for `outputs:` ONLY.
            //
            // An `outputs:` connection authored on a domain network root is read
            // at parse time by `lunco-usd-sim-domain` and becomes an equation inside
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
                && prim_facts.is_domain_root
                && attribute.is_network_boundary_output
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
                && prim_facts.is_domain_root
                && attribute.feeds_internal_network_input
            {
                continue;
            }
            if attr.starts_with("inputs:") && prim_facts.is_domain_root && !has_modelica {
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
            let scale = attribute.scale;
            let offset = attribute.offset;

            // A PARAMETER IS AN INPUT WITH A CONSTANT INSTEAD OF A CONNECTION.
            // An `inputs:` port with no wire into it is authored data — `float
            // inputs:kv = 1.2` — and it is the ONLY way USD reaches a model's
            // parameters. Collected here (the one pass that already enumerates
            // every `inputs:` port with the composed reader in hand) and applied
            // by `seed_usd_input_defaults` once the model exists.
            let sources = &attribute.connections;
            if sources.is_empty() {
                // An unconnected `outputs:` is just a declared port, not a parameter.
                if forward.is_some() {
                    continue;
                }
                if let Some(v) = attribute.value {
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
                            src_prim,
                            src_conn,
                        ))
                    })
                    .flatten()
                    .copied();
                let generated_alias_present = generated_alias.is_some();
                let (mut start_element, mut src_conn) = if let Some((wrapper, alias)) =
                    generated_alias
                {
                    (wrapper, alias.to_string())
                } else {
                    // A source path is absolute in the composed USD stage. An
                    // instance-local source must resolve in the sink's instance,
                    // but a scene-authored source (for example a kinematic landing
                    // target) intentionally lives outside that instance. Resolve
                    // the local namespace first, then the authored scene namespace.
                    // This keeps duplicated assets isolated without making a
                    // scene-level connection depend on which asset consumes it.
                    let source_key = (prim_path.stage_handle.id(), sink_instance, src_prim);
                    let scene_source_key = (prim_path.stage_handle.id(), None, src_prim);
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
                        lunco_cosim_core::EARTH_MOUNT_X_CONNECTOR
                            | lunco_cosim_core::EARTH_MOUNT_Y_CONNECTOR
                            | lunco_cosim_core::EARTH_MOUNT_Z_CONNECTOR
                    )
                {
                    earth_direction_required.insert(start_element);
                }

                // ── The SOURCE side of the runtime-output indirection ────────
                // A vessel's `outputs:drive_left` is not stored on the vessel
                // prim: `OutputPorts` realises it as a child `Port` entity, and
                // that is where the authored controller writes. The sink side above has
                // always redirected onto that child; reading one had no such hop,
                // so a wire whose SOURCE is a vessel actuator port resolved to the
                // vessel entity, found no port of that name, and delivered its
                // default forever.
                //
                // MEASURED on `scenes/tests/solar_domain_nested_ref.usda`: the
                // rover's `throttle` reached 1.0 and the authored controller wrote both
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
                            wiring
                                .outputs
                                .get(start_element)
                                .ok()
                                .and_then(|outputs| outputs.get(&src_conn))
                        })
                    {
                        start_element = port_entity;
                        src_conn = lunco_cosim_core::PORT_NAME.to_string();
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
                    && lunco_physics::force_ports::is_physics_force_port(sink_conn)
                    && matches!(
                        wiring.predicted_bodies.get(entity),
                        Ok(avian3d::prelude::RigidBody::Dynamic)
                    )
                    && wiring.realtime_safe.get(start_element).is_err()
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
                        .map(|port| (port, lunco_cosim_core::PORT_NAME.to_string()))
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
                let connection = SimConnection {
                    start_element,
                    start_connector: src_conn.to_string(),
                    start_is_input,
                    end_element,
                    end_connector,
                    scale,
                    offset,
                };
                let name = format!("UsdWire {src} -> {}.{sink_conn}", prim_path.path);
                let identity = connection_identity(&connection);
                if let Some(previous) = previous_edges
                    .get_mut(&identity)
                    .and_then(std::collections::VecDeque::pop_front)
                {
                    if same_connection(&previous.connection, &connection)
                        && previous.name.as_deref() == Some(name.as_str())
                    {
                        continue;
                    }
                    commands.entity(previous.entity).try_despawn();
                }
                commands.spawn((
                    connection,
                    UsdWiredConnection,
                    // Keep the immutable USD fact on the derived runtime edge.
                    // The generic binder has no USD dependency, but its terminal
                    // diagnostics still need to name the authored source and
                    // sink that must be repaired.
                    Name::new(name),
                    // A derived edge is a pure cache of USD wiring — every peer
                    // re-derives it from the same stage, so it must never carry
                    // network identity. `Local` makes that ownership explicit;
                    // session identity admission leaves it without a global id,
                    // so the wiring cache cannot trigger its own identity-based
                    // rebuild gate.
                    // See docs/architecture/42-ui-frame-discipline.md §6.
                    lunco_core::Provenance::Local,
                ));
            }
        }
    }

    for edges in previous_edges.into_values() {
        for edge in edges {
            commands.entity(edge.entity).try_despawn();
        }
    }

    facts_cache
        .0
        .retain(|key, _| active_cache_keys.contains(key));

    // Publish the authored parameters — but ONLY where they changed. This runs on
    // every structural change (any prim spawning anywhere re-runs the whole pass),
    // and `seed_usd_input_defaults` reacts to `Changed`. Re-inserting an identical
    // map would fire `Changed` anyway and re-seed the model, clobbering a value a
    // script had since written through `SetPorts` — an autopilot's `engage` would
    // silently snap back to its authored default the next time anything spawned.
    for (entity, map) in defaults {
        if wiring
            .defaults
            .get(entity)
            .map(|d| d.0 != map)
            .unwrap_or(true)
        {
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
pub(super) fn causal_participants_changed(
    arrivals: Query<
        (),
        Or<(
            Added<SimConnection>,
            Changed<SimConnection>,
            Changed<ConnectionBinding>,
            Added<ModelicaModel>,
            Added<lunco_port_core::CausalStateSink>,
        )>,
    >,
    mut removed_connections: RemovedComponents<SimConnection>,
    mut removed_bindings: RemovedComponents<ConnectionBinding>,
    mut removed_models: RemovedComponents<ModelicaModel>,
    mut removed_sinks: RemovedComponents<lunco_port_core::CausalStateSink>,
    revision: Res<lunco_cosim_core::BindingRevision>,
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
pub(super) fn derive_causal_barrier_participants(world: &mut World) {
    let modelica_entities: bevy::ecs::entity::EntityHashSet = world
        .query_filtered::<Entity, With<ModelicaModel>>()
        .iter(world)
        .collect();

    let stateful_sinks: bevy::ecs::entity::EntityHashSet = world
        .query_filtered::<Entity, With<lunco_port_core::CausalStateSink>>()
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
        .get_resource::<lunco_cosim_core::BindingRevision>()
        .is_some_and(|revision| revision.sealed)
        && bindings_terminal;

    let mut projection = world.resource_mut::<lunco_core_runtime::SimulationBarrierParticipants>();
    if topology_ready {
        projection.replace(participants);
    } else {
        projection.topology_ready = false;
    }
}
