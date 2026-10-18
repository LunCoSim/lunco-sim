//! Immutable policy preparation and versioned, non-blocking UI publication.

use super::*;
use lunco_core_runtime::{AsyncWorkAdmission, AsyncWorkKey, AsyncWorkKind, AsyncWorkPriority};
use lunco_exposure_core::{ExposureState, ExposureSurface};
use lunco_hooks::{HookInvocation, RegisteredHook};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, mpsc},
};

const OWNER: u128 = u128::from_le_bytes(*b"runtime-exposure");

#[derive(Clone, PartialEq, Eq)]
pub(super) enum InputIdentity {
    Runtime {
        entity: Entity,
        subject: Option<GlobalEntityId>,
        control_owner: String,
    },
    Camera(u32),
}
impl InputIdentity {
    fn subject(&self) -> Option<GlobalEntityId> {
        match self {
            Self::Runtime { subject, .. } => *subject,
            Self::Camera(_) => None,
        }
    }
}

#[derive(Clone)]
pub(super) struct ProjectionInput {
    pub namespace: String,
    pub identity: InputIdentity,
    pub facts: HookValue,
    pub context: lunco_core::RuntimeExecutionContext,
    pub telemetry: Vec<PublicTelemetryValue>,
    pub driven: Option<DrivenVessel>,
}

impl ProjectionInput {
    fn same_source(&self, other: &Self) -> bool {
        self.namespace == other.namespace
            && self.identity == other.identity
            && self.facts == other.facts
            && self.telemetry == other.telemetry
            && self.driven == other.driven
            && self.context.route == other.context.route
            && self.context.phase == other.context.phase
            && self.context.clock == other.context.clock
            && self.context.producer == other.context.producer
    }
}

struct PreparedSurface {
    namespace: String,
    identity: InputIdentity,
    result: Result<ExposureSurface, String>,
}
struct Flight {
    epoch: u64,
    key: AsyncWorkKey,
    identities: Vec<(String, InputIdentity)>,
    receiver: Mutex<mpsc::Receiver<Result<Vec<PreparedSurface>, String>>>,
}
#[derive(PartialEq, Eq)]
struct SourceStamp {
    hooks: u64,
    stage: Option<u64>,
    scene: Option<Entity>,
}

/// One current job, one coalesced input and one immutable admitted snapshot per
/// surface. Source changes release publication and comparison state without
/// waiting for retired workers; shared admission bounds outstanding execution.
#[derive(Resource)]
pub(super) struct RuntimeExposurePreparation {
    epoch: u64,
    operation: u64,
    source: Option<SourceStamp>,
    pending: BTreeMap<String, ProjectionInput>,
    admitted_inputs: BTreeMap<String, Arc<ProjectionInput>>,
    identities: BTreeMap<String, InputIdentity>,
    failed: HashSet<String>,
    flight: Option<Flight>,
    refresh_camera: bool,
}
impl Default for RuntimeExposurePreparation {
    fn default() -> Self {
        Self {
            epoch: 1,
            operation: 0,
            source: None,
            pending: BTreeMap::new(),
            admitted_inputs: BTreeMap::new(),
            identities: BTreeMap::new(),
            failed: HashSet::new(),
            flight: None,
            refresh_camera: false,
        }
    }
}
impl SceneResource for RuntimeExposurePreparation {
    fn clear_scene(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.flight = None;
        self.source = None;
        self.pending.clear();
        self.admitted_inputs.clear();
        self.identities.clear();
        self.failed.clear();
        self.refresh_camera = true;
    }
}
impl RuntimeExposurePreparation {
    pub(super) fn queue(&mut self, input: ProjectionInput, exposures: &mut EngineExposures) {
        let changed = self.identities.get(&input.namespace) != Some(&input.identity);
        if changed || !exposures.surfaces.contains_key(&input.namespace) {
            self.admitted_inputs.remove(&input.namespace);
            self.failed.remove(&input.namespace);
            exposures.replace_surface(
                &input.namespace,
                ExposureSurface {
                    state: ExposureState::Pending,
                    subject: input.identity.subject(),
                    ..Default::default()
                },
            );
        }
        self.identities
            .insert(input.namespace.clone(), input.identity.clone());
        if !self.failed.contains(&input.namespace) {
            self.pending.insert(input.namespace.clone(), input);
        }
    }
    pub(super) fn attach_driven(&mut self, namespace: &str, vessel: DrivenVessel) {
        if let Some(input) = self.pending.get_mut(namespace) {
            input.driven = Some(vessel);
        }
    }
    pub(super) fn retire(&mut self, namespace: &str, exposures: &mut EngineExposures) {
        self.pending.remove(namespace);
        self.admitted_inputs.remove(namespace);
        self.identities.remove(namespace);
        self.failed.remove(namespace);
        exposures.remove_surface(namespace);
    }
    pub(super) fn invalidate(&mut self, admission: &mut AsyncWorkAdmission) {
        if let Some(flight) = &self.flight {
            admission.cancel_queued(flight.key);
        }
        self.clear_scene();
    }

    fn fail(
        &mut self,
        namespace: &str,
        identity: &InputIdentity,
        message: String,
        exposures: &mut EngineExposures,
    ) {
        if self.identities.get(namespace) != Some(identity) {
            return;
        }
        warn!(
            surface_id = namespace,
            "[runtime-ui] preparation failed: {message}"
        );
        self.failed.insert(namespace.to_owned());
        self.pending.remove(namespace);
        exposures.replace_surface(
            namespace,
            ExposureSurface {
                state: ExposureState::Failed(message),
                subject: identity.subject(),
                ..Default::default()
            },
        );
    }
    fn collect(&mut self, exposures: &mut EngineExposures) {
        let Some(flight) = &mut self.flight else {
            return;
        };
        let result = flight
            .receiver
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .try_recv();
        if matches!(result, Err(mpsc::TryRecvError::Empty)) {
            return;
        }
        let flight = self
            .flight
            .take()
            .expect("a completed UI flight remains owned until collection");
        if flight.epoch != self.epoch {
            return;
        }
        match result {
            Ok(Ok(surfaces)) => {
                for prepared in surfaces {
                    if self.identities.get(&prepared.namespace) != Some(&prepared.identity) {
                        continue;
                    }
                    match prepared.result {
                        Ok(surface) => exposures.replace_surface(&prepared.namespace, surface),
                        Err(message) => {
                            self.fail(&prepared.namespace, &prepared.identity, message, exposures)
                        }
                    }
                }
            }
            other => {
                let message = match other {
                    Ok(Err(message)) => message,
                    _ => "UI policy worker ended without a result".to_owned(),
                };
                for (name, identity) in flight.identities {
                    self.fail(&name, &identity, message.clone(), exposures);
                }
            }
        }
    }
}

/// Service completions independently of capture cadence and simulation holds.
pub(super) fn begin_preparation(
    time: Res<Time<Real>>,
    scene: Res<SceneMountState>,
    stage: Option<Res<lunco_usd_bevy_scene::UsdStageRevision>>,
    camera: Res<lunco_usd_bevy_camera::camera_switch::CameraSelectionStatus>,
    mut owner: ResMut<RuntimeExposurePreparation>,
    mut admission: ResMut<AsyncWorkAdmission>,
    mut exposures: ResMut<EngineExposures>,
    mut refresh: ResMut<ExposureRefresh>,
) {
    let stamp = SourceStamp {
        hooks: lunco_hooks::generation(),
        stage: stage.map(|stage| stage.0),
        scene: scene.active_root(),
    };
    if owner
        .source
        .as_ref()
        .is_some_and(|previous| previous != &stamp)
    {
        let names = owner.identities.keys().cloned().collect::<Vec<_>>();
        owner.invalidate(&mut admission);
        for name in names {
            exposures.replace_surface(
                &name,
                ExposureSurface {
                    state: ExposureState::Pending,
                    ..Default::default()
                },
            );
        }
        refresh.first_update = true;
    }
    owner.source = Some(stamp);
    if owner.refresh_camera {
        owner.refresh_camera = false;
        queue_camera_exposure(
            &mut owner,
            &mut exposures,
            &camera,
            presentation_execution_context(&time, lunco_core::RuntimePhase::Event),
            camera.last_changed().get(),
        );
    }
    owner.collect(&mut exposures);
}

/// Submit through shared bounded admission; queue pressure retains owned inputs.
pub(super) fn submit_preparation(
    mut owner: ResMut<RuntimeExposurePreparation>,
    mut admission: ResMut<AsyncWorkAdmission>,
    mut exposures: ResMut<EngineExposures>,
) {
    if owner.flight.is_some() || owner.pending.is_empty() {
        return;
    }
    // Bevy change ticks can include presentation writes of equal values.
    // These policies project source facts rather than animate on clock ticks;
    // retain the admitted caller context until their actual inputs change.
    // Compare after attach_driven so derived vessel data participates too.
    let inputs = std::mem::take(&mut owner.pending)
        .into_values()
        .filter(|input| {
            owner
                .admitted_inputs
                .get(&input.namespace)
                .is_none_or(|previous| !input.same_source(previous))
        })
        .map(Arc::new)
        .collect::<Vec<_>>();
    if inputs.is_empty() {
        return;
    }
    owner.operation = owner.operation.wrapping_add(1);
    let key = AsyncWorkKey::new(
        AsyncWorkKind::VisualizationPreparation,
        owner.epoch,
        OWNER,
        lunco_hooks::generation(),
        owner.operation,
    );
    let identities = inputs
        .iter()
        .map(|input| (input.namespace.clone(), input.identity.clone()))
        .collect();
    let admitted_inputs = inputs.clone();
    let payload = Arc::new(Mutex::new(Some(inputs)));
    let owned_payload = Arc::clone(&payload);
    let visibility = lunco_hooks::get(RUNTIME_UI_VISIBILITY_HOOK);
    let properties = lunco_hooks::get(RUNTIME_UI_PROPERTIES_HOOK);
    let (sender, receiver) = mpsc::channel();
    let job = move || {
        let _span = bevy::log::info_span!("runtime_ui_policy_preparation").entered();
        let inputs = owned_payload
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let result = inputs
            .ok_or_else(|| "UI preparation lost its input ownership".to_owned())
            .map(|inputs| {
                inputs
                    .into_iter()
                    .map(|input| {
                        let result =
                            prepare_surface(&input, visibility.as_deref(), properties.as_deref());
                        PreparedSurface {
                            namespace: input.namespace.clone(),
                            identity: input.identity.clone(),
                            result,
                        }
                    })
                    .collect()
            });
        let _ = sender.send(result);
    };
    let admitted = submit_job(&mut admission, key, job);
    if let Err(error) = admitted {
        drop(admitted_inputs);
        let inputs = payload
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(inputs) = inputs {
            for input in inputs {
                if error == lunco_core_runtime::AsyncWorkRejection::QueueFull {
                    let input = Arc::unwrap_or_clone(input);
                    owner.pending.insert(input.namespace.clone(), input);
                } else {
                    owner.fail(
                        &input.namespace,
                        &input.identity,
                        format!("UI preparation admission failed: {error:?}"),
                        &mut exposures,
                    );
                }
            }
        }
        return;
    }
    for input in admitted_inputs {
        owner.admitted_inputs.insert(input.namespace.clone(), input);
    }
    owner.flight = Some(Flight {
        epoch: owner.epoch,
        key,
        identities,
        receiver: Mutex::new(receiver),
    });
}

fn submit_job(
    admission: &mut AsyncWorkAdmission,
    key: AsyncWorkKey,
    job: impl FnOnce() + Send + 'static,
) -> Result<(), lunco_core_runtime::AsyncWorkRejection> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        admission.submit(AsyncWorkPriority::Interactive, key, job)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let pool = bevy::tasks::AsyncComputeTaskPool::try_get()
            .ok_or(lunco_core_runtime::AsyncWorkRejection::NativeDispatcherUnavailable)?;
        let permit = admission.admit_external(AsyncWorkPriority::Interactive, key)?;
        pool.spawn_local(async move {
            job();
            drop(permit);
        })
        .detach();
        Ok(())
    }
}

fn invoke(
    hook: Option<&RegisteredHook>,
    id: &str,
    input: &ProjectionInput,
) -> Result<HookValue, String> {
    let hook = hook.ok_or_else(|| format!("no policy is registered for {id}"))?;
    hook.invoke(&HookInvocation::with_context(
        std::slice::from_ref(&input.facts),
        input.context,
    ))
    .map_err(|error| error.to_string())
}
fn prepare_surface(
    input: &ProjectionInput,
    visibility: Option<&RegisteredHook>,
    properties: Option<&RegisteredHook>,
) -> Result<ExposureSurface, String> {
    let mut surface = ExposureSurface {
        subject: input.identity.subject(),
        ..Default::default()
    };
    if matches!(input.identity, InputIdentity::Camera(_)) {
        surface.visible = true;
        for key in ["active_name", "active_label"] {
            let value = input
                .facts
                .get(key)
                .and_then(HookValue::as_str)
                .ok_or_else(|| format!("camera preparation omitted {key}"))?;
            surface.set_property(key, value);
        }
    } else {
        let _span = bevy::log::info_span!("exposure_runtime_surface_visibility").entered();
        let value = invoke(visibility, RUNTIME_UI_VISIBILITY_HOOK, input)?;
        surface.visible = value
            .get("visible")
            .and_then(|value| match value {
                HookValue::Bool(value) => Some(*value),
                _ => None,
            })
            .ok_or("visibility policy must return a map with boolean visible")?;
    }
    let value = {
        let _span = bevy::log::info_span!("exposure_runtime_surface_properties").entered();
        invoke(properties, RUNTIME_UI_PROPERTIES_HOOK, input)?
    };
    let HookValue::Map(values) = value else {
        return Err("properties policy must return a typed map".into());
    };
    for (name, value) in values {
        surface.set_property(
            &name,
            hook_value_to_exposure(value, &input.namespace, &name)?,
        );
    }
    if let Some(vessel) = &input.driven {
        publish_vessel_values(&mut surface, vessel, &input.telemetry);
    }
    Ok(surface)
}

pub(super) fn on_twin_closed(
    trigger: On<lunco_workspace::TwinClosed>,
    mut owner: ResMut<RuntimeExposurePreparation>,
    mut admission: ResMut<AsyncWorkAdmission>,
    mut exposures: ResMut<EngineExposures>,
    mut refresh: ResMut<ExposureRefresh>,
) {
    if !trigger.was_active {
        return;
    }
    owner.invalidate(&mut admission);
    exposures.clear_scene();
    refresh.clear_scene();
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input(identity: InputIdentity) -> ProjectionInput {
        ProjectionInput {
            namespace: "view".into(),
            identity,
            facts: HookValue::map([] as [(&str, HookValue); 0]),
            context: presentation_execution_context(
                &Time::<Real>::default(),
                lunco_core::RuntimePhase::Event,
            ),
            telemetry: Vec::new(),
            driven: None,
        }
    }
    fn flight(
        owner: &mut RuntimeExposurePreparation,
        identity: InputIdentity,
    ) -> mpsc::Sender<Result<Vec<PreparedSurface>, String>> {
        let (sender, receiver) = mpsc::channel();
        owner.pending.clear();
        owner.flight = Some(Flight {
            epoch: owner.epoch,
            key: AsyncWorkKey::new(
                AsyncWorkKind::VisualizationPreparation,
                owner.epoch,
                OWNER,
                1,
                1,
            ),
            identities: vec![("view".into(), identity)],
            receiver: Mutex::new(receiver),
        });
        sender
    }
    fn ready(identity: InputIdentity, value: &str) -> PreparedSurface {
        let mut surface = ExposureSurface {
            visible: true,
            ..Default::default()
        };
        surface.set_property("marker", value);
        PreparedSurface {
            namespace: "view".into(),
            identity,
            result: Ok(surface),
        }
    }
    #[test]
    fn ui_projection_equal_facts_skip_admission_but_retirement_releases_the_snapshot() {
        let mut world = World::new();
        let mut owner = RuntimeExposurePreparation::default();
        let mut registry = EngineExposures::default();
        let initial = input(InputIdentity::Camera(1));
        owner
            .admitted_inputs
            .insert("view".into(), Arc::new(initial.clone()));
        registry.replace_surface(
            "view",
            ready(initial.identity.clone(), "current").result.unwrap(),
        );
        let mut later = initial.clone();
        later.context.time_seconds = Some(2.0);
        later.context.delta_seconds = Some(0.5);
        later.context.sequence = Some(20);
        owner
            .identities
            .insert("view".into(), initial.identity.clone());
        owner.queue(later, &mut registry);
        world.insert_resource(AsyncWorkAdmission::default());
        world.insert_resource(registry);
        world.insert_resource(owner);
        let mut system = IntoSystem::into_system(submit_preparation);
        system.initialize(&mut world);
        system.run((), &mut world).unwrap();
        let owner = world.resource::<RuntimeExposurePreparation>();
        assert_eq!(
            owner.operation, 0,
            "identical facts must not request another worker job"
        );
        assert!(owner.pending.is_empty() && owner.flight.is_none());
        assert_eq!(
            world.resource::<EngineExposures>().surfaces["view"].properties["marker"].render(),
            "current"
        );
        let mut changed = initial.clone();
        changed.facts = HookValue::map([("value", HookValue::Float(1.0))]);
        assert!(
            !initial.same_source(&changed),
            "a changed source requires a new projection"
        );
        changed = initial.clone();
        changed.context.phase = lunco_core::RuntimePhase::Preparation;
        assert!(
            !initial.same_source(&changed),
            "caller semantics are part of the source identity"
        );
        world.resource_scope(|world, mut owner: Mut<RuntimeExposurePreparation>| {
            world
                .resource_mut::<EngineExposures>()
                .remove_surface("view");
            owner.queue(
                initial.clone(),
                &mut world.resource_mut::<EngineExposures>(),
            );
            assert!(
                owner.admitted_inputs.is_empty(),
                "a missing publication must wake its producer"
            );
            owner.retire("view", &mut world.resource_mut::<EngineExposures>());
            assert!(owner.admitted_inputs.is_empty());
            owner.queue(initial, &mut world.resource_mut::<EngineExposures>());
            assert_eq!(
                owner.pending.len(),
                1,
                "a new lifetime must prepare even equal facts"
            );
        });
    }

    #[test]
    fn ui_projection_stale_completion_cannot_republish_a_retired_epoch() {
        let mut owner = RuntimeExposurePreparation::default();
        let mut registry = EngineExposures::default();
        owner.queue(input(InputIdentity::Camera(1)), &mut registry);
        let sender = flight(&mut owner, InputIdentity::Camera(1));
        owner.invalidate(&mut AsyncWorkAdmission::default());
        assert!(
            owner.flight.is_none(),
            "retirement must release the old result receiver immediately"
        );
        owner.queue(input(InputIdentity::Camera(2)), &mut registry);
        assert!(
            sender
                .send(Ok(vec![ready(InputIdentity::Camera(1), "retired")]))
                .is_err(),
            "late workers cannot retain a publication destination"
        );
        owner.collect(&mut registry);
        assert!(owner.flight.is_none());
        assert_eq!(registry.surfaces["view"].state, ExposureState::Pending);
        assert!(registry.surfaces["view"].properties.is_empty());
        assert_eq!(owner.pending.len(), 1);
    }
    #[test]
    fn ui_projection_keeps_valid_samples_while_coalescing_newer_inputs() {
        let identity = InputIdentity::Runtime {
            entity: Entity::from_bits(1),
            subject: None,
            control_owner: "local".into(),
        };
        let mut owner = RuntimeExposurePreparation::default();
        let mut registry = EngineExposures::default();
        owner.queue(input(identity.clone()), &mut registry);
        let sender = flight(&mut owner, identity.clone());
        owner.queue(input(identity.clone()), &mut registry);
        owner.queue(input(identity.clone()), &mut registry);
        sender.send(Ok(vec![ready(identity, "sample")])).unwrap();
        owner.collect(&mut registry);
        assert_eq!(
            registry.surfaces["view"].properties["marker"].render(),
            "sample"
        );
        assert_eq!(owner.pending.len(), 1);
    }
    #[test]
    fn ui_projection_failure_is_terminal_until_its_source_changes() {
        let mut owner = RuntimeExposurePreparation::default();
        let mut registry = EngineExposures::default();
        owner.queue(input(InputIdentity::Camera(1)), &mut registry);
        owner.fail(
            "view",
            &InputIdentity::Camera(1),
            "invalid".into(),
            &mut registry,
        );
        owner.queue(input(InputIdentity::Camera(1)), &mut registry);
        assert!(owner.pending.is_empty());
        assert_eq!(
            registry.surfaces["view"].state,
            ExposureState::Failed("invalid".into())
        );
        owner.queue(input(InputIdentity::Camera(2)), &mut registry);
        assert_eq!(registry.surfaces["view"].state, ExposureState::Pending);
        assert_eq!(owner.pending.len(), 1);
    }
    #[test]
    fn ui_projection_queue_pressure_returns_owned_inputs_without_execution() {
        let mut world = World::new();
        let mut admission = AsyncWorkAdmission::default();
        admission.set_limits(1, 1).unwrap();
        admission
            .submit(
                AsyncWorkPriority::Interactive,
                AsyncWorkKey::new(AsyncWorkKind::VisualizationPreparation, 7, 8, 9, 10),
                || {},
            )
            .unwrap();
        let mut registry = EngineExposures::default();
        let mut owner = RuntimeExposurePreparation::default();
        owner.queue(input(InputIdentity::Camera(1)), &mut registry);
        world.insert_resource(admission);
        world.insert_resource(registry);
        world.insert_resource(owner);
        let mut system = IntoSystem::into_system(submit_preparation);
        system.initialize(&mut world);
        system.run((), &mut world).unwrap();
        let owner = world.resource::<RuntimeExposurePreparation>();
        assert!(owner.flight.is_none());
        assert_eq!(owner.pending.len(), 1);
    }
    #[test]
    fn ui_projection_property_lowering_rejects_loss_and_partial_collections() {
        assert!(hook_value_to_exposure(HookValue::UInt(u64::MAX), "view", "value").is_err());
        assert!(hook_value_to_exposure(HookValue::Int(i64::MAX), "view", "value").is_err());
        assert!(
            hook_value_to_exposure(
                HookValue::Array(vec![HookValue::Float(1.0), HookValue::Float(f64::NAN)]),
                "view",
                "value"
            )
            .is_err()
        );
        assert_eq!(
            hook_value_to_exposure(HookValue::UInt(53), "view", "value").unwrap(),
            ExposureValue::Number(53.0)
        );
    }
}
