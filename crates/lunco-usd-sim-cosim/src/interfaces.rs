//! Resolved Modelica declarations admitted before the causal surface is ready.

use super::preparations::ModelicaPreparations;
use super::*;
use lunco_modelica_ast::resolved_interface::{ModelicaCausality, ModelicaScalarType};
use lunco_modelica_runtime::{
    ModelicaInterfaceOutcome, ModelicaPreparationId, ModelicaPreparationOutcome,
};

#[derive(Clone, Debug, PartialEq)]
struct SourceLease {
    generation: u64,
    handle: Option<Handle<ModelicaSource>>,
    uri: String,
    source: String,
    class: String,
    session: u64,
    roots: Vec<String>,
    authored: std::collections::BTreeMap<String, u64>,
    native_inputs: BTreeSet<String>,
    native_outputs: BTreeSet<String>,
}

impl SourceLease {
    fn authored_bindings(&self) -> lunco_modelica_runtime::ModelicaAuthoredBindings {
        lunco_modelica_runtime::ModelicaAuthoredBindings {
            values: self
                .authored
                .iter()
                .map(|(name, bits)| (name.clone(), f64::from_bits(*bits)))
                .collect(),
            external_inputs: self.native_inputs.clone(),
        }
    }
}

#[derive(Component)]
pub(super) struct QueuedInterface(SourceLease);

#[derive(Component)]
pub(super) struct PendingInterface {
    request: ModelicaPreparationId,
    lease: SourceLease,
}

#[derive(Component, Clone)]
pub(super) struct ResolvedUsdModelicaInterface {
    lease: SourceLease,
    pub(super) outcome: std::sync::Arc<ModelicaInterfaceOutcome>,
    pub(super) inputs: BTreeSet<String>,
    pub(super) outputs: BTreeSet<String>,
    pub(super) parameters: BTreeSet<String>,
    pub(super) native_inputs: BTreeSet<String>,
    pub(super) native_outputs: BTreeSet<String>,
}

impl ResolvedUsdModelicaInterface {
    pub(super) fn matches(&self, session: u64, uri: &str, source: &str) -> bool {
        self.lease.session == session && self.lease.uri == uri && self.lease.source == source
    }

    pub(super) fn matches_authored(&self, defaults: Option<&UsdInputDefaults>) -> bool {
        self.lease.authored == authored_snapshot(defaults)
    }
}

fn authored_snapshot(
    defaults: Option<&UsdInputDefaults>,
) -> std::collections::BTreeMap<String, u64> {
    defaults
        .into_iter()
        .flat_map(|defaults| &defaults.0)
        .map(|(name, value)| (name.clone(), value.to_bits()))
        .collect()
}

fn source_lease(world: &World, entity: Entity) -> Result<Option<SourceLease>, String> {
    let Some(generation) = world
        .get_resource::<lunco_core::SceneTransitionCoordinator>()
        .and_then(lunco_core::SceneTransitionCoordinator::lifecycle_generation)
    else {
        return Ok(None);
    };
    let authored = authored_snapshot(world.get::<UsdInputDefaults>(entity));
    let (native_inputs, native_outputs) = native_ports(world, entity)?;
    if let Some(pending) = world.get::<PendingModelicaSource>(entity) {
        if world
            .resource::<AssetServer>()
            .load_state(&pending.handle)
            .is_failed()
        {
            return Err(format!(
                "failed to load Modelica source `{}`",
                pending.asset_path
            ));
        }
        let Some(source) = world
            .resource::<Assets<ModelicaSource>>()
            .get(&pending.handle)
        else {
            return Ok(None);
        };
        let name = source.interface.model_name.as_ref().ok_or_else(|| {
            format!(
                "Modelica source `{}` contains no runnable class",
                pending.asset_path
            )
        })?;
        let class = match source
            .interface
            .within
            .as_deref()
            .filter(|within| !within.is_empty())
        {
            Some(within) if !name.starts_with(&format!("{within}.")) => format!("{within}.{name}"),
            _ => name.clone(),
        };
        return Ok(Some(SourceLease {
            generation,
            handle: Some(pending.handle.clone()),
            uri: pending.asset_path.clone(),
            source: source.text.clone(),
            class,
            session: pending.session_id,
            roots: source
                .interface
                .required_source_roots
                .iter()
                .cloned()
                .collect(),
            authored,
            native_inputs,
            native_outputs,
        }));
    }
    let Some(source) = world.get::<GeneratedModelicaSource>(entity) else {
        return Ok(None);
    };
    if let Some(error) = &source.projection_error {
        return Err(error.clone());
    }
    let Some(model) = world.get::<ModelicaModel>(entity) else {
        return Ok(None);
    };
    if model.document.is_unassigned() {
        return Ok(None);
    }
    let documents = world.get_resource::<lunco_doc_bevy::DocumentRegistry<lunco_modelica_document::ModelicaDocument>>()
        .ok_or("generated Modelica document registry is unavailable")?;
    let Some(host) = documents.host(model.document) else {
        return Ok(None);
    };
    let document = host.document();
    // The document owner supplies the same source identity consumed by live
    // compilation. Wait for its source projection before preparing that lease.
    if document.source() != source.source {
        return Ok(None);
    }
    Ok(Some(SourceLease {
        generation,
        handle: None,
        uri: document.origin().session_uri(),
        source: source.source.clone(),
        class: model.model_name.clone(),
        session: model.session_id,
        roots: source.source_roots.clone(),
        authored,
        native_inputs,
        native_outputs,
    }))
}

fn native_ports(
    world: &World,
    entity: Entity,
) -> Result<(BTreeSet<String>, BTreeSet<String>), String> {
    let registry = world
        .get_resource::<lunco_port_core::ports::PortRegistry>()
        .ok_or("Modelica interface admission requires the registered port owners")?;
    let participant = world
        .get_resource::<lunco_cosim::ports::ParticipantPortBackend>()
        .ok_or("Modelica interface admission requires the participant port backend")?;
    let mut inputs = BTreeSet::new();
    let mut outputs = BTreeSet::new();
    for owner in registry.entity_port_owners(world, entity) {
        if owner.backend == participant.0 || owner.backend == registry.input_storage_backend() {
            continue;
        }
        use lunco_port_core::ports::PortDirection;
        if matches!(owner.direction, PortDirection::In | PortDirection::InOut) {
            inputs.insert(owner.name.clone());
        }
        if matches!(owner.direction, PortDirection::Out | PortDirection::InOut) {
            outputs.insert(owner.name);
        }
    }
    Ok((inputs, outputs))
}

fn resolved_surface(
    world: &World,
    entity: Entity,
    lease: SourceLease,
    outcome: ModelicaInterfaceOutcome,
) -> Result<ResolvedUsdModelicaInterface, String> {
    if outcome.entity != entity
        || outcome.session_id != lease.session
        || outcome.doc_uri != lease.uri
        || outcome.interface.model_name != lease.class
    {
        return Err("Modelica interface preparation returned a different source lease".into());
    }
    let native_inputs = lease.native_inputs.clone();
    let native_outputs = lease.native_outputs.clone();
    let mut inputs = BTreeSet::new();
    let mut outputs = BTreeSet::new();
    let mut parameters = BTreeSet::new();
    for (name, declaration) in &outcome.interface.declarations {
        if declaration.is_parameter {
            parameters.insert(name.clone());
        }
        let numeric = declaration
            .scalar_type
            .is_some_and(|kind| kind != ModelicaScalarType::String);
        if numeric && declaration.causality == ModelicaCausality::Input {
            inputs.insert(name.clone());
        }
        if declaration.observable {
            outputs.insert(name.clone());
        }
    }
    if let Some(contract) = world.get::<UsdModelicaPortContract>(entity) {
        for name in &contract.inputs {
            let actual_input = outcome
                .interface
                .declarations
                .get(name)
                .is_some_and(|declaration| declaration.causality == ModelicaCausality::Input);
            if actual_input && !inputs.contains(name) {
                return Err(format!(
                    "USD numeric input `{name}` has a nonnumeric Modelica type"
                ));
            }
            if !inputs.contains(name) && !parameters.contains(name) && !native_inputs.contains(name)
            {
                return Err(format!(
                    "USD input `{name}` has no checked Modelica input, parameter, or registered native owner"
                ));
            }
        }
        for name in &contract.outputs {
            if !outputs.contains(name) && !native_outputs.contains(name) {
                return Err(format!(
                    "USD output `{name}` has no checked Modelica observable or registered native owner"
                ));
            }
        }
    }
    // The worker classified and validated every supplied numeric binding before
    // deriving this exact instance interface. Native-only values stayed with
    // their registered owner and never became Modelica modifications.
    Ok(ResolvedUsdModelicaInterface {
        lease,
        outcome: std::sync::Arc::new(outcome),
        inputs,
        outputs,
        parameters,
        native_inputs,
        native_outputs,
    })
}

fn reject(world: &mut World, entity: Entity, error: String) {
    let subject = world
        .get::<UsdPrimPath>(entity)
        .map(|path| path.path.clone())
        .unwrap_or_else(|| format!("{entity:?}"));
    // Interface preparation performs strict compilation before a live actor
    // exists. Its terminal failure therefore owns the same simulation fault
    // as a failed live participant constructor, without waiting for readiness.
    world.init_resource::<lunco_core::RuntimeFaults>();
    world.resource_mut::<lunco_core::RuntimeFaults>().raise(
        "modelica-compile-failed",
        Some(entity),
        subject.clone(),
        error.clone(),
    );
    if let Some(mut component) = world.get_mut::<SimComponent>(entity) {
        component.status = SimStatus::Error(error.clone());
        component.is_stepping = false;
        component.inputs.clear();
        component.outputs.clear();
        component.parameters.clear();
    }
    if let Some(mut model) = world.get_mut::<ModelicaModel>(entity) {
        model.last_error = Some(error.clone());
        model.is_compiling = false;
        model.is_stepping = false;
        model.paused = true;
        model.resume_after_compile = false;
    }
    if let Ok(mut owner) = world.get_entity_mut(entity) {
        // Model removal delegates worker, variables and publisher retirement
        // to their existing lifecycle observers; the terminal SimComponent
        // remains so the readiness owner closes its compile ticket as failed.
        owner.remove::<ModelicaModel>();
        owner.remove::<(
            PendingModelicaSource,
            PendingInterface,
            QueuedInterface,
            ResolvedUsdModelicaInterface,
            lunco_modelica_runtime::ModelicaResolvedInterface,
            ModelicaSignalLayout,
            lunco_port_core::ScalarPortContracts,
            DeclaredOutputPorts,
            lunco_port_core::PortSurfacePending,
        )>();
    }
    error!("[usd-cosim] {subject}: {error}");
    world.write_message(lunco_modelica_runtime::ModelicaNotice {
        level: lunco_modelica_runtime::NoticeLevel::Error,
        text: format!("{subject}: {error}"),
    });
    world.trigger(lunco_telemetry_core::TelemetryEvent {
        name: MODEL_CONFIGURATION_INVALID.into(),
        source: 0,
        severity: lunco_telemetry_core::Severity::Error,
        data: lunco_telemetry_core::TelemetryValue::String(error),
        timestamp: 0.0,
        sim_secs: 0.0,
        sim_tick: 0,
    });
}

fn publish(
    world: &mut World,
    entity: Entity,
    surface: ResolvedUsdModelicaInterface,
) -> Result<(), String> {
    let mut layout = world
        .get::<ModelicaSignalLayout>(entity)
        .cloned()
        .unwrap_or_default();
    if surface.lease.handle.is_some() {
        layout.metadata.clear();
    }
    layout
        .metadata
        .extend(
            surface
                .outcome
                .interface
                .declarations
                .iter()
                .filter_map(|(name, declaration)| {
                    (declaration.description.is_some() || declaration.unit.is_some()).then(|| {
                        (
                            name.clone(),
                            lunco_modelica_ast::ast_extract::ModelicaVariableMetadata {
                                description: declaration.description.clone(),
                                unit: declaration.unit.clone(),
                            },
                        )
                    })
                }),
        );
    let mut contracts = layout.port_contracts()?;
    if let Some(current) = world.get::<lunco_port_core::ScalarPortContracts>(entity) {
        for name in surface.native_inputs.union(&surface.native_outputs) {
            if !surface.inputs.contains(name) && !surface.outputs.contains(name) {
                if let Some(contract) = current.get(name) {
                    contracts.set(name.clone(), contract.clone())?;
                }
            }
        }
    }
    let mut component = world
        .get_mut::<SimComponent>(entity)
        .ok_or("Modelica participant retired before interface publication")?;
    let mut inputs = lunco_cosim_core::ScalarPortMap::default();
    for name in &surface.inputs {
        inputs.declare(name.clone());
        if let Some(value) = component.inputs.get(name) {
            inputs.set(name, *value);
        }
    }
    component.inputs = inputs;
    component
        .parameters
        .retain(|name, _| surface.parameters.contains(name));
    drop(component);
    let runtime_interface =
        lunco_modelica_runtime::ModelicaResolvedInterface(surface.outcome.clone());
    world
        .entity_mut(entity)
        .insert((
            DeclaredOutputPorts::new(surface.outputs.iter().cloned().collect()),
            contracts,
            layout,
            runtime_interface,
            surface,
        ))
        .remove::<(PendingInterface, lunco_port_core::PortSurfacePending)>();
    Ok(())
}

fn source_discovery_pending(world: &mut World) -> bool {
    world.contains_resource::<SceneLoadInFlight>()
        || world
            .get_resource::<lunco_core_runtime::SimulationProgress>()
            .is_some_and(|progress| {
                progress.contains(lunco_core_runtime::SimulationProgressKey::usd_domain_discovery())
            })
        || world
            .query_filtered::<Entity, (
                With<lunco_usd_sim_domain::DomainNetworkProjectionPending>,
                Without<SimComponent>,
            )>()
            .iter(world)
            .next()
            .is_some()
}

/// Admission is one source-root batch. No request sees a partially committed
/// cohort merely because an asset or source-root worker completed first.
fn admit_interface_roots(world: &mut World, entities: &[(Entity, String)]) -> Result<bool, String> {
    let roots: BTreeSet<_> = entities
        .iter()
        .filter_map(|(entity, _)| world.get::<QueuedInterface>(*entity))
        .flat_map(|queued| queued.0.roots.iter().cloned())
        .collect();
    if !world.contains_resource::<ModelicaChannels>() {
        return Err("Modelica preparation channels are unavailable".into());
    }
    if !world.contains_resource::<lunco_modelica_source_roots::SourceRootRegistry>() {
        return if roots.is_empty() {
            Ok(true)
        } else {
            Err("Modelica source-root registry is unavailable".into())
        };
    }
    world.resource_scope(
        |world, mut registry: Mut<lunco_modelica_source_roots::SourceRootRegistry>| {
            lunco_modelica_source_roots::admit_compile_roots(
                &mut registry,
                roots,
                world.resource::<ModelicaChannels>(),
            )?;
            Ok(!registry.roots.values().any(|root| {
                matches!(
                    root.state,
                    lunco_modelica_source_roots::LoadState::Loading { .. }
                )
            }))
        },
    )
}

fn receive_interfaces(world: &mut World, entities: &[(Entity, String)]) {
    for (entity, _) in entities {
        let Some(pending) = world.get::<PendingInterface>(*entity) else {
            continue;
        };
        let request = pending.request;
        if !world.resource::<ModelicaPreparations>().ready(request) {
            continue;
        }
        let lease = pending.lease.clone();
        let Some(result) = world.resource_mut::<ModelicaPreparations>().take(request) else {
            continue;
        };
        match source_lease(world, *entity) {
            Ok(Some(current)) if current == lease => {}
            Ok(_) => {
                retire_interface_preparation(world, *entity);
                continue;
            }
            Err(error) => {
                reject(world, *entity, error);
                continue;
            }
        }
        let result = result
            .and_then(|outcome| {
                let ModelicaPreparationOutcome::Interface(outcome) = outcome else {
                    return Err(
                        "Modelica interface request returned an initialization result".into(),
                    );
                };
                resolved_surface(world, *entity, lease, outcome)
            })
            .and_then(|surface| publish(world, *entity, surface));
        if let Err(error) = result {
            reject(world, *entity, error);
        }
    }
}

pub(super) fn publish_loaded_modelica_interfaces(world: &mut World) {
    if source_discovery_pending(world) {
        return;
    }
    let mut query = world.query_filtered::<(Entity, &UsdPrimPath), (
        With<SimComponent>,
        With<lunco_port_core::PortSurfacePending>,
        Or<(With<PendingModelicaSource>, With<GeneratedModelicaSource>)>,
    )>();
    let mut entities: Vec<_> = query
        .iter(world)
        .map(|(entity, path)| (entity, path.path.clone()))
        .collect();
    entities.sort_by(|a, b| a.1.cmp(&b.1));
    receive_interfaces(world, &entities);
    let mut waiting_for_sources = false;
    for (entity, _) in &entities {
        if world.get::<PendingInterface>(*entity).is_some()
            || world.get::<QueuedInterface>(*entity).is_some()
            || world
                .get::<lunco_port_core::PortSurfacePending>(*entity)
                .is_none()
        {
            continue;
        }
        match source_lease(world, *entity) {
            Ok(Some(lease)) => {
                world.entity_mut(*entity).insert(QueuedInterface(lease));
            }
            Ok(None) => waiting_for_sources = true,
            Err(error) => reject(world, *entity, error),
        }
    }
    if !entities
        .iter()
        .any(|(entity, _)| world.get::<QueuedInterface>(*entity).is_some())
    {
        return;
    }
    match admit_interface_roots(world, &entities) {
        Ok(true) if !waiting_for_sources => {}
        Ok(_) => return,
        Err(error) => {
            for (entity, _) in entities {
                if world.get::<QueuedInterface>(entity).is_some() {
                    reject(world, entity, error.clone());
                }
            }
            return;
        }
    }
    for (entity, _) in entities {
        let Some(QueuedInterface(lease)) = world.entity_mut(entity).take::<QueuedInterface>()
        else {
            continue;
        };
        match source_lease(world, entity) {
            Ok(Some(current)) if current == lease => {}
            Ok(_) => {
                retire_interface_preparation(world, entity);
                continue;
            }
            Err(error) => {
                reject(world, entity, error);
                continue;
            }
        }
        let request = match world
            .resource_mut::<ModelicaPreparations>()
            .allocate(lease.generation)
        {
            Ok(request) => request,
            Err(error) => {
                reject(world, entity, error);
                continue;
            }
        };
        let command = ModelicaCommand::PrepareInterface {
            request_id: request,
            entity,
            session_id: lease.session,
            model_name: lease.class.clone(),
            source: lease.source.clone(),
            doc_uri: lease.uri.clone(),
            extra_sources: Vec::new(),
            parameter_overrides: Vec::new(),
            authored_bindings: lease.authored_bindings(),
        };
        if world
            .resource::<ModelicaChannels>()
            .tx
            .send(command)
            .is_err()
        {
            world.resource_scope(|world, mut preparations: Mut<ModelicaPreparations>| {
                preparations.cancel(request, world.get_resource::<ModelicaChannels>())
            });
            reject(
                world,
                entity,
                "Modelica interface worker channel closed".into(),
            );
            continue;
        }
        world
            .entity_mut(entity)
            .insert(PendingInterface { request, lease });
    }
}

pub(super) fn cancel_removed_interface(
    trigger: On<Remove<PendingInterface>>,
    pending: Query<&PendingInterface>,
    channels: Option<Res<ModelicaChannels>>,
    mut preparations: ResMut<ModelicaPreparations>,
) {
    if let Ok(pending) = pending.get(trigger.entity) {
        preparations.cancel(pending.request, channels.as_deref());
    }
}

pub(super) fn cancel_removed_source<T: Component>(trigger: On<Remove<T>>, mut commands: Commands) {
    commands
        .entity(trigger.entity)
        .try_remove::<(PendingInterface, QueuedInterface)>();
}

fn retire_interface_preparation(world: &mut World, entity: Entity) {
    if let Ok(mut owner) = world.get_entity_mut(entity) {
        owner.remove::<(
            PendingInterface,
            QueuedInterface,
            ResolvedUsdModelicaInterface,
            lunco_modelica_runtime::ModelicaResolvedInterface,
        )>();
        if owner.contains::<PendingModelicaSource>() || owner.contains::<GeneratedModelicaSource>()
        {
            owner.insert(lunco_port_core::PortSurfacePending);
        }
    }
}

pub(super) fn invalidate_inserted_source<T: Component>(
    trigger: On<Insert<T>>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    commands.queue(move |world: &mut World| retire_interface_preparation(world, entity));
}

pub(super) fn invalidate_preparing_bindings(
    trigger: On<Insert<UsdInputDefaults>>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    commands.queue(move |world: &mut World| {
        let preparing = world.get::<PendingInterface>(entity).is_some()
            || world.get::<QueuedInterface>(entity).is_some()
            || world.get::<PendingModelicaSource>(entity).is_some()
            || world
                .get::<ResolvedUsdModelicaInterface>(entity)
                .is_some_and(|interface| {
                    world.get::<ModelicaModel>(entity).is_some_and(|model| {
                        !model.is_compiled && model.session_id == interface.lease.session
                    })
                });
        if preparing {
            retire_interface_preparation(world, entity);
        }
    });
}

pub(super) fn invalidate_changed_source_assets(
    mut events: MessageReader<AssetEvent<ModelicaSource>>,
    pending: Query<(Entity, &PendingModelicaSource)>,
    mut commands: Commands,
) {
    let changed: HashSet<_> = events
        .read()
        .filter_map(|event| match event {
            AssetEvent::Modified { id } | AssetEvent::Removed { id } => Some(*id),
            _ => None,
        })
        .collect();
    for (entity, source) in &pending {
        if changed.contains(&source.handle.id()) {
            commands.queue(move |world: &mut World| retire_interface_preparation(world, entity));
        }
    }
}
