use super::session::{Description, FatalRegistry, Session, Values};
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender, bounded};
use lunco_assets_core::registered_read::AssetReadTarget;
use lunco_command_contracts::{Ack, OpId};
use lunco_core::{
    AppSceneExt, Command, RuntimeFaults, SceneOwned, SceneResource, on_command, register_commands,
};
use lunco_core_runtime::async_work::ExternalWorkPermit;
use lunco_core_runtime::{
    AsyncWorkAdmission, AsyncWorkKey, AsyncWorkKind, AsyncWorkPriority, SECS_PER_TICK, SimTick,
    SimulationCompletions, SimulationProgress, SimulationProgressKey, SimulationProgressOwner,
    TickClock,
};
use lunco_cosim_core::{SimComponent, SimStatus};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Resource, Clone, Copy)]
pub struct FmuLimits {
    pub max_instances: usize,
    pub max_archive_bytes: usize,
    pub max_unpacked_bytes: u64,
    pub max_event_iterations: usize,
}
impl Default for FmuLimits {
    fn default() -> Self {
        Self {
            max_instances: 32,
            max_archive_bytes: 64 * 1024 * 1024,
            max_unpacked_bytes: 256 * 1024 * 1024,
            max_event_iterations: 1000,
        }
    }
}

/// External FMU admission. Initial values use FMU starts unless explicitly
/// overridden. Communication is one physics tick by default, with intentional
/// subrates expressed as positive integer tick counts rather than another Hz clock.
#[Command]
pub struct LoadFmu {
    pub source: String,
    pub name: String,
    #[serde(default = "one_tick")]
    #[reflect(default = "one_tick")]
    pub communication_ticks: u32,
    #[serde(default)]
    #[reflect(default)]
    pub initial_inputs: Values,
    #[serde(default)]
    #[reflect(default)]
    pub parameters: Values,
}
fn one_tick() -> u32 {
    1
}

/// Pause one native participant's child clock without changing master transport.
#[Command]
pub struct SetFmuPaused {
    #[authz_target]
    pub target: Entity,
    pub paused: bool,
}

#[on_command(SetFmuPaused)]
fn on_set_fmu_paused(
    trigger: On<SetFmuPaused>,
    mut participants: Query<&mut FmuParticipant>,
    suspended: Query<(), With<lunco_core::SimulationObjectPaused>>,
) -> Result<Ack, String> {
    let request = trigger.event();
    if !request.paused && suspended.contains(request.target) {
        warn!("[fmi] rejected resume: participant belongs to a suspended coupled system");
        return Err("cannot resume one participant of a suspended coupled system".to_owned());
    }
    let mut participant = participants.get_mut(request.target).map_err(|_| {
        warn!("[fmi] rejected pause: target is not a native FMU participant");
        "pause target is not a native FMU participant".to_owned()
    })?;
    if participant.paused != request.paused {
        participant.paused = request.paused;
    }
    Ok(Ack::new(OpId::new()))
}

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct FmuParticipant {
    pub source: String,
    pub communication_ticks: u32,
    pub time_s: f64,
    pub paused: bool,
    /// Declared FMU capability; a coupling algorithm must still implement restoration.
    pub fmu_can_restore_state: bool,
    pub source_revision: u64,
    pub output_dependencies: BTreeMap<String, Vec<String>>,
    #[reflect(ignore)]
    clock: TickClock,
    #[reflect(ignore)]
    accepted_tick: u64,
    #[reflect(ignore)]
    pending: bool,
    #[reflect(ignore)]
    interval_inputs: Option<Values>,
    #[reflect(ignore)]
    lease: Arc<AtomicBool>,
}
impl Drop for FmuParticipant {
    fn drop(&mut self) {
        self.lease.store(false, Ordering::Release);
    }
}

#[derive(Resource)]
struct Channels {
    tx: Sender<Work>,
    rx: Receiver<Reply>,
}
#[derive(Resource, Default)]
struct Owners(BTreeSet<Entity>);
impl SceneResource for Owners {}
enum Work {
    Load {
        entity: Entity,
        name: String,
        target: AssetReadTarget,
        server: AssetServer,
        inputs: Values,
        parameters: Values,
        ticks: u32,
        limits: FmuLimits,
        lease: Arc<AtomicBool>,
        permit: ExternalWorkPermit,
    },
    Step {
        entity: Entity,
        start_tick: u64,
        stop_tick: u64,
        inputs: Values,
    },
}
enum ResultData {
    Loaded {
        description: Description,
        source_revision: u64,
    },
    Stepped {
        tick: u64,
        time: f64,
        outputs: Values,
    },
}
struct Reply {
    entity: Entity,
    result: Result<ResultData, String>,
    _permit: Option<ExternalWorkPermit>,
}
struct OwnedSession {
    session: Session,
    lease: Arc<AtomicBool>,
}

fn key(entity: Entity) -> SimulationProgressKey {
    SimulationProgressKey {
        owner: SimulationProgressOwner::FmiExecution,
        operation_id: entity.to_bits(),
    }
}

#[on_command(LoadFmu)]
fn on_load_fmu(
    trigger: On<LoadFmu>,
    mut commands: Commands,
    channels: Res<Channels>,
    server: Res<AssetServer>,
    roots: Option<Res<lunco_assets_core::TwinRoots>>,
    limits: Res<FmuLimits>,
    admission: Res<AsyncWorkAdmission>,
    mut owners: ResMut<Owners>,
    mut progress: ResMut<SimulationProgress>,
) -> Result<Ack, String> {
    let request = trigger.event();
    if request.name.trim().is_empty()
        || request.name.contains('\0')
        || request.communication_ticks == 0
        || owners.0.len() >= limits.max_instances
    {
        warn!("[fmi] rejected FMU admission: invalid name/period or instance capacity exhausted");
        return Err(
            "FMU requires a name, positive communication ticks and available instance capacity"
                .into(),
        );
    }
    for batch in [&request.initial_inputs, &request.parameters] {
        let mut names = BTreeSet::new();
        if batch
            .iter()
            .any(|(name, value)| name.is_empty() || !value.is_finite() || !names.insert(name))
        {
            warn!("[fmi] rejected FMU admission: input/parameter batch is invalid");
            return Err(
                "FMU input/parameter batches require unique names and finite values".into(),
            );
        }
    }
    let native = lunco_storage::file_uri_to_path(&request.source).map_err(|e| e.to_string())?;
    let target = if let Some(path) = native {
        AssetReadTarget::File(path)
    } else if std::path::Path::new(&request.source).is_absolute() {
        AssetReadTarget::File(request.source.clone().into())
    } else {
        AssetReadTarget::Registered(
            lunco_assets_core::asset_path::load_asset_path(
                &request.source,
                None,
                roots.as_deref(),
                None,
            )
            .map_err(|e| e.to_string())?,
        )
    };
    let entity = commands.spawn_empty().id();
    let permit = admission
        .admit_external(
            AsyncWorkPriority::SimulationRequired,
            AsyncWorkKey::new(
                AsyncWorkKind::FmiPreparation,
                0,
                entity.to_bits() as u128,
                0,
                0,
            ),
        )
        .map_err(|e| {
            commands.entity(entity).despawn();
            format!("FMU preparation not admitted: {e:?}")
        })?;
    let lease = Arc::new(AtomicBool::new(true));
    let work = Work::Load {
        entity,
        name: request.name.clone(),
        target,
        server: server.clone(),
        inputs: request.initial_inputs.clone(),
        parameters: request.parameters.clone(),
        ticks: request.communication_ticks,
        limits: *limits,
        lease: lease.clone(),
        permit,
    };
    if channels.tx.try_send(work).is_err() {
        commands.entity(entity).despawn();
        return Err("FMU worker queue is unavailable or full".into());
    }
    // Runtime participants enter the same session-owned identity admission as
    // other command-created objects, making their canonical ports discoverable.
    commands.entity(entity).insert((
        SceneOwned,
        lunco_core::Provenance::Authoritative,
        Name::new(request.name.clone()),
        FmuParticipant {
            source: request.source.clone(),
            communication_ticks: request.communication_ticks,
            time_s: 0.0,
            paused: false,
            fmu_can_restore_state: false,
            source_revision: 0,
            output_dependencies: BTreeMap::new(),
            clock: TickClock::default(),
            accepted_tick: 0,
            pending: true,
            interval_inputs: None,
            lease,
        },
        SimComponent {
            model_name: request.name.clone(),
            status: SimStatus::Compiling,
            ..Default::default()
        },
    ));
    progress.acquire(
        key(entity),
        format!("Preparing external FMU {}", request.name),
    );
    owners.0.insert(entity);
    Ok(Ack::new(OpId::new()))
}
register_commands!(on_load_fmu, on_set_fmu_paused);

fn worker(rx: Receiver<Work>, tx: Sender<Reply>) {
    let mut sessions = BTreeMap::<Entity, OwnedSession>::new();
    let mut fatal_registry = FatalRegistry::new();
    loop {
        sessions.retain(|_, session| session.lease.load(Ordering::Acquire));
        // Successful retired models release their metadata too. Fatal model
        // identities remain poisoned for this native host's process lifetime.
        fatal_registry.retain(|_, flag| flag.get() || std::rc::Rc::strong_count(flag) > 1);
        let work = match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(work) => work,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };
        let entity = match &work {
            Work::Load { entity, .. } | Work::Step { entity, .. } => *entity,
        };
        if matches!(&work, Work::Load { lease, .. } if !lease.load(Ordering::Acquire)) {
            continue;
        }
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match work {
            Work::Load {
                entity,
                name,
                target,
                server,
                inputs,
                parameters,
                ticks,
                limits,
                lease,
                permit,
            } => {
                let result = (|| {
                    let bytes =
                        bevy::tasks::block_on(target.read(&server, limits.max_archive_bytes))?;
                    let (session, description) = Session::load(
                        &bytes,
                        &name,
                        &inputs,
                        &parameters,
                        ticks as f64 * SECS_PER_TICK,
                        limits.max_unpacked_bytes,
                        limits.max_event_iterations,
                        &mut fatal_registry,
                    )?;
                    if !lease.load(Ordering::Acquire) {
                        return Err("FMU owner retired during preparation".into());
                    }
                    sessions.insert(entity, OwnedSession { session, lease });
                    Ok(ResultData::Loaded {
                        description,
                        source_revision: lunco_hash::fnv1a64(&bytes),
                    })
                })();
                (entity, result, Some(permit))
            }
            Work::Step {
                entity,
                start_tick,
                stop_tick,
                inputs,
            } => {
                let result = sessions
                    .get_mut(&entity)
                    .ok_or_else(|| "FMU session is unavailable".to_owned())
                    .and_then(|owned| {
                        owned
                            .session
                            .step(
                                SimTick(start_tick).seconds(),
                                SimTick(stop_tick).seconds(),
                                &inputs,
                            )
                            .and_then(|outputs| {
                                owned.session.time().map(|time| ResultData::Stepped {
                                    tick: stop_tick,
                                    time,
                                    outputs,
                                })
                            })
                    });
                (entity, result, None)
            }
        }));
        let (entity, result, permit) = match outcome {
            Ok(result) => result,
            Err(_) => (
                entity,
                Err("FMI library panicked while processing this participant".into()),
                None,
            ),
        };
        if result.is_err() {
            sessions.remove(&entity);
        }
        if tx
            .send(Reply {
                entity,
                result,
                _permit: permit,
            })
            .is_err()
        {
            break;
        }
    }
}

fn fault(
    entity: Entity,
    model: &mut FmuParticipant,
    component: &mut SimComponent,
    faults: &mut RuntimeFaults,
    detail: String,
) {
    model.paused = true;
    model.pending = false;
    component.is_stepping = false;
    component.status = SimStatus::Error(detail.clone());
    faults.raise(
        "fmi-execution-failed",
        Some(entity),
        component.model_name.clone(),
        detail,
    );
}

fn receive(
    mut commands: Commands,
    channels: Res<Channels>,
    mut models: Query<(Entity, &mut FmuParticipant, &mut SimComponent)>,
    mut progress: ResMut<SimulationProgress>,
    mut faults: ResMut<RuntimeFaults>,
    mut removed: RemovedComponents<FmuParticipant>,
    mut owners: ResMut<Owners>,
) {
    for entity in removed.read() {
        progress.release(key(entity));
        owners.0.remove(&entity);
    }
    // One serial worker returns requests in dispatch order. A live owner is
    // fenced by Bevy's generational entity identity; removed owners consume no result.
    loop {
        let reply = match channels.rx.try_recv() {
            Ok(reply) => reply,
            Err(crossbeam_channel::TryRecvError::Empty) => break,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                // Losing a native host cannot leave an accepted transaction
                // holding the shared clock forever. Completed samples above
                // remain valid; outstanding requests fail at their owner.
                for (entity, mut model, mut component) in &mut models {
                    if model.pending {
                        fault(
                            entity,
                            &mut model,
                            &mut component,
                            &mut faults,
                            "FMU worker disconnected before completing its transaction".into(),
                        );
                        progress.release(key(entity));
                    }
                }
                break;
            }
        };
        let Ok((_, mut model, mut component)) = models.get_mut(reply.entity) else {
            progress.release(key(reply.entity));
            continue;
        };
        match reply.result {
            Ok(ResultData::Loaded {
                description,
                source_revision,
            }) => {
                commands
                    .entity(reply.entity)
                    .try_insert(description.contracts);
                component.model_name = description.name;
                component.inputs = description.inputs.into_iter().collect();
                component.outputs = description.outputs.into_iter().collect();
                component.status = SimStatus::Running;
                model.fmu_can_restore_state = description.rollback;
                model.output_dependencies = description.dependencies;
                model.source_revision = source_revision;
                model.pending = false;
            }
            Ok(ResultData::Stepped {
                tick,
                time,
                outputs,
            }) => {
                let expected = model
                    .accepted_tick
                    .checked_add(u64::from(model.communication_ticks));
                if expected != Some(tick) || !model.pending {
                    fault(
                        reply.entity,
                        &mut model,
                        &mut component,
                        &mut faults,
                        "FMU returned an unexpected communication transaction".into(),
                    );
                } else {
                    model.accepted_tick = tick;
                    model.time_s = time;
                    model.pending = false;
                    component.is_stepping = false;
                    component.outputs = outputs.into_iter().collect();
                }
            }
            Err(detail) => fault(
                reply.entity,
                &mut model,
                &mut component,
                &mut faults,
                detail,
            ),
        }
        progress.release(key(reply.entity));
    }
}

fn dispatch(
    master: Res<SimTick>,
    channels: Res<Channels>,
    mut models: Query<(Entity, &mut FmuParticipant, &mut SimComponent)>,
    suspended: Query<(), With<lunco_core::SimulationObjectPaused>>,
    mut progress: ResMut<SimulationProgress>,
    mut faults: ResMut<RuntimeFaults>,
) {
    if faults.active() {
        return;
    }
    let mut ordered: Vec<_> = models.iter_mut().collect();
    ordered.sort_unstable_by_key(|(entity, _, _)| entity.to_bits());
    for (entity, mut model, mut component) in ordered {
        let active = !suspended.contains(entity)
            && !model.paused
            && matches!(component.status, SimStatus::Running);
        if let Err(detail) = model.clock.observe(*master, active) {
            fault(
                entity,
                &mut model,
                &mut component,
                &mut faults,
                detail.into(),
            );
            return;
        }
        if !active {
            continue;
        }
        if model.pending {
            fault(
                entity,
                &mut model,
                &mut component,
                &mut faults,
                "FMU step is still pending at a new master boundary".into(),
            );
            return;
        }
        if model.interval_inputs.is_none() {
            let mut inputs: Values = component
                .inputs
                .iter()
                .map(|(name, value)| (name.to_owned(), *value))
                .collect();
            inputs.sort_by(|a, b| a.0.cmp(&b.0));
            model.interval_inputs = Some(inputs);
        }
        let Some(stop_tick) = model
            .accepted_tick
            .checked_add(u64::from(model.communication_ticks))
        else {
            fault(
                entity,
                &mut model,
                &mut component,
                &mut faults,
                "FMU communication tick range exhausted".into(),
            );
            return;
        };
        if model.clock.elapsed_ticks() < stop_tick {
            continue;
        }
        let work = Work::Step {
            entity,
            start_tick: model.accepted_tick,
            stop_tick,
            inputs: model
                .interval_inputs
                .take()
                .expect("admitted interval owns its input sample"),
        };
        if channels.tx.try_send(work).is_err() {
            fault(
                entity,
                &mut model,
                &mut component,
                &mut faults,
                "FMU worker queue is unavailable or full".into(),
            );
            return;
        }
        model.pending = true;
        component.is_stepping = true;
        progress.acquire(
            key(entity),
            format!("Advancing external FMU {}", component.model_name),
        );
    }
}

pub struct FmiPlugin;

/// Application composition orders backend receipt commits through this set.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FmiCompletionSet;
impl Plugin for FmiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FmuLimits>();
        app.init_scene_resource::<Owners>();
        let count = app.world().resource::<FmuLimits>().max_instances;
        let capacity = count.saturating_mul(2).max(1);
        let (tx, requests) = bounded(capacity);
        let (responses, rx) = bounded(capacity);
        if !app.is_plugin_added::<lunco_core_runtime::SimulationCompletionPlugin>() {
            app.add_plugins(lunco_core_runtime::SimulationCompletionPlugin);
        }
        let completion_lease = app
            .world_mut()
            .resource_mut::<lunco_core_runtime::SimulationCompletionSources>()
            .register(&rx);
        if let Err(error) = std::thread::Builder::new()
            .name("fmi-worker".into())
            .spawn(move || {
                let _completion_lease = completion_lease;
                worker(requests, responses)
            })
        {
            // The disconnected sender rejects admission; no accepted command
            // can acquire a hold for a worker that never started.
            error!("[fmi] could not start native worker: {error}");
        }
        app.insert_resource(Channels { tx, rx });
        app.register_type::<FmuParticipant>();
        register_all_commands(app);
        app.add_systems(SimulationCompletions, receive.in_set(FmiCompletionSet));
        app.add_systems(
            FixedUpdate,
            dispatch
                .after(lunco_cosim_core::schedule::CosimSet::Propagate)
                .run_if(lunco_time::simulation_is_running),
        );
    }
}
