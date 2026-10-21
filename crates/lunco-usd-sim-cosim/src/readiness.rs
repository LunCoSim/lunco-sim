//! Declaring what a loading scene is still waiting on.
//!
//! [`lunco_readiness`] holds the registry and the policy; `lunco_physics` does
//! the freezing. This module publishes scene, program and physics-admission
//! facts from their authoritative state:
//!
//! | Wait | Open while | Scope |
//! |---|---|---|
//! | [`kinds::SCENE_LOAD`] | the stage or its projected participants are still settling | world |
//! | [`kinds::PROGRAM_COMPILE`] | an entity's Modelica model has not compiled | its owning physical subtree |
//! | [`kinds::PROGRAM_FAILED`] | a program's preparation or solver has failed, including rejection before a solver exists | its owning physical subtree |
//! | [`kinds::PARTICIPANT_INIT`] | an active Modelica model lacks a valid initialized t=0 snapshot (entity), or an authored rigid body has not been admitted to Avian (world) | entity subtree or world |
//!
//! # Why reconcile systems rather than events
//!
//! A ticket opened on an event has to be closed on the matching event, on every
//! path — including the ones that end in a failed asset load, a scene reload
//! halfway through a compile, or a despawn. Missing one leaks a hold that freezes
//! the world until the producer closes the ticket; an overdue ticket is a
//! terminal runtime fault and never releases the world implicitly.
//!
//! These systems instead derive the wait from state that is *already* the truth:
//! `SceneLoadInFlight` and unresolved `UsdSceneAwaitingStage` prims for the scene, a
//! `ModelicaModel` for solver preparation, `SimComponent` for pre-solver
//! preparation and rejection, and physics-admission markers. Each frame the
//! wait either still describes the world or it does not. A program component's
//! removal also closes its exact ticket, because its physical subject can
//! survive the program.

use bevy::prelude::*;
use lunco_modelica_runtime::{ModelicaModel, ModelicaVariables};
use lunco_readiness::{ReadinessRegistry, ReadinessTicket, Subject, kinds};

use lunco_cosim_core::{SimComponent, UsdSourcedCosim};
use lunco_physics::{PhysicsHolds, PhysicsInitializationPending, PhysicsObjectPaused};
use lunco_usd_avian_contracts::ShouldBeDynamic;
use lunco_usd_bevy_runtime_core::scene::SceneLoadInFlight;
use lunco_usd_bevy_scene::{UsdPrimPath, UsdSceneAwaitingStage};

/// The open world-scoped scene-load wait, if a scene is loading.
#[derive(Resource)]
struct SceneLoadWait {
    ticket: ReadinessTicket,
}

/// The open world-scoped wait for the USD → Avian admission boundary.
///
/// `ShouldBeDynamic` and `PhysicsStatePending` are the authoritative markers
/// that a body is still held before dynamic admission. Initialization policy
/// pending state is intentionally included in the latter boundary so a bad
/// authored pose cannot enter the solver between projection and validation.
#[derive(Resource)]
struct PhysicsAdmissionWait {
    ticket: ReadinessTicket,
}

/// One program's preparation, failure or initial-snapshot wait. The ticket's
/// physical subject may outlive the program, so removal closes its exact lease.
#[derive(Component)]
struct ProgramReadinessWait {
    ticket: ReadinessTicket,
    kind: &'static str,
    label: String,
    /// The physical owner held while the program is unavailable.  Modelica
    /// programs are commonly authored below a rigid body, so the program
    /// entity itself is not the thing whose initial state must remain fixed.
    owner: Entity,
}

fn finish_program_readiness_wait(
    removed: On<Remove<ProgramReadinessWait>>,
    waits: Query<&ProgramReadinessWait>,
    mut registry: ResMut<ReadinessRegistry>,
) {
    if let Ok(wait) = waits.get(removed.entity) {
        registry.finish(wait.ticket);
    }
}

/// Hold the world while the USD stage itself is being mounted.
///
/// This ticket owns only the stage transaction: before the asset is loaded and
/// while prims still wait for their stage. Modelica compilation is tracked by
/// entity-scoped tickets below, and Avian joint/wheel/differential admission is
/// deliberately local. Tying this world ticket to the native binding epoch
/// would make a local physics gate prevent the whole world from becoming ready.
fn track_scene_load(
    in_flight: Option<Res<SceneLoadInFlight>>,
    awaiting: Query<(), With<UsdSceneAwaitingStage>>,
    wait: Option<Res<SceneLoadWait>>,
    mut registry: ResMut<ReadinessRegistry>,
    mut commands: Commands,
) {
    let stage_loading = in_flight.is_some() || !awaiting.is_empty();
    let loading = scene_still_loading(stage_loading);
    match (loading, wait) {
        (true, None) => {
            let label = in_flight
                .map(|g| g.path.clone())
                .unwrap_or_else(|| "deferred prims".into());
            let ticket = registry.begin(Subject::World, kinds::SCENE_LOAD, label);
            commands.insert_resource(SceneLoadWait { ticket });
        }
        (false, Some(wait)) => {
            registry.finish(wait.ticket);
            commands.remove_resource::<SceneLoadWait>();
        }
        _ => {}
    }
}

fn scene_still_loading(stage_loading: bool) -> bool {
    stage_loading
}

/// Freeze the physical owner of a USD program during its preparation lifecycle
/// while it compiles or an active participant awaits its first successful
/// communication point. A deliberately paused, compiled model is ready without
/// a live step.
///
/// This is the descent-lander race, closed: the entity exists and has mass and a
/// collider long before the model that is supposed to fly it has been through the
/// compiler. Until it has, the object is not a vehicle — it is a rock with a
/// pending appointment — and it must not be falling.
///
/// The `ModelicaModel` lifecycle owns solver preparation. A bind-published
/// [`SimComponent`] intentionally remains `Compiling` until its first solver
/// tick, so the model's source state closes the compile wait and its first
/// successful communication point closes participant initialization. Waiting
/// on the wrapper status would conflate those separate lifecycle facts. Before
/// a solver exists, the component's preparation or terminal error owns the wait.
///
/// The wait is deliberately attached to the owning physical subtree rather than
/// the whole world. A cold compiler must not stop unrelated rovers, terrain
/// streaming, or already-ready physics. The owner is derived from the authored
/// entity hierarchy and the runtime rigid-body schema; there is no vehicle-name
/// or program-name special case. A standalone program with no rigid-body
/// ancestor holds its own entity, preserving the original participant scope.
fn track_program_readiness(
    initializing: Query<(), With<lunco_cosim_core::initialization::CoupledInitializationPending>>,
    programs: Query<
        (
            Entity,
            Option<&ModelicaModel>,
            Option<&ModelicaVariables>,
            Option<&SimComponent>,
        ),
        (
            With<UsdSourcedCosim>,
            Or<(With<ModelicaModel>, With<SimComponent>)>,
        ),
    >,
    waits: Query<(Entity, &ProgramReadinessWait)>,
    parents: Query<&ChildOf>,
    rigid_bodies: Query<(), With<avian3d::prelude::RigidBody>>,
    mut registry: ResMut<ReadinessRegistry>,
    mut commands: Commands,
) {
    for (entity, model, variables, component) in &programs {
        let (label, kind) = match (model, component) {
            (Some(model), component) => {
                let kind = modelica_wait_kind(model, variables, component);
                (
                    model.model_name.as_str(),
                    if kind == Some(kinds::PROGRAM_FAILED) {
                        kind
                    } else if initializing.contains(entity) {
                        Some(kinds::PARTICIPANT_INIT)
                    } else {
                        kind
                    },
                )
            }
            (None, Some(component)) => {
                let kind = if matches!(component.status, lunco_cosim_core::SimStatus::Error(_)) {
                    Some(kinds::PROGRAM_FAILED)
                } else if initializing.contains(entity) {
                    Some(kinds::PARTICIPANT_INIT)
                } else if matches!(component.status, lunco_cosim_core::SimStatus::Compiling) {
                    Some(kinds::PROGRAM_COMPILE)
                } else {
                    None
                };
                (component.model_name.as_str(), kind)
            }
            (None, None) => continue,
        };
        let wait = waits.get(entity).ok().map(|(_, wait)| wait);
        let Some(kind) = kind else {
            if let Some(wait) = wait {
                registry.finish(wait.ticket);
                commands.entity(entity).try_remove::<ProgramReadinessWait>();
            }
            continue;
        };
        let owner = owning_physics_entity(entity, &parents, &rigid_bodies);
        if wait.is_some_and(|wait| kind == wait.kind && owner == wait.owner && label == wait.label)
        {
            continue;
        }
        if let Some(wait) = wait {
            registry.finish(wait.ticket);
        }
        let ticket = registry.begin(Subject::Entity(owner), kind, label.to_owned());
        commands.entity(entity).try_insert(ProgramReadinessWait {
            ticket,
            kind,
            owner,
            label: label.to_owned(),
        });
    }
    for (entity, wait) in &waits {
        if programs.get(entity).is_err() {
            registry.finish(wait.ticket);
            commands.entity(entity).try_remove::<ProgramReadinessWait>();
        }
    }
}

/// Keep scene-owned scenario hooks closed until every authored dynamic body has
/// crossed the USD/Avian admission boundary.
///
/// The body projector owns the truth here; this system only publishes it as a
/// readiness fact. That prevents a fixed-step scenario from observing one body
/// with zero kinematic velocity while another body has already received its
/// authored release velocity. The wait is world-scoped because a scene's
/// startup program must see one coherent initial condition across its
/// articulated participants.
fn track_physics_admission(
    awaiting_dynamic_admission: Query<
        &UsdPrimPath,
        (With<ShouldBeDynamic>, Without<PhysicsObjectPaused>),
    >,
    still_pending: Query<
        &UsdPrimPath,
        (
            Or<(
                With<lunco_core::PhysicsStatePending>,
                With<PhysicsInitializationPending>,
            )>,
            Without<PhysicsObjectPaused>,
        ),
    >,
    physics_state_pending: Query<
        &UsdPrimPath,
        (
            With<lunco_core::PhysicsStatePending>,
            Without<PhysicsObjectPaused>,
        ),
    >,
    initialization_pending: Query<
        &UsdPrimPath,
        (
            With<PhysicsInitializationPending>,
            Without<PhysicsObjectPaused>,
        ),
    >,
    body_admission_hold: Option<Res<PhysicsHolds>>,
    wait: Option<Res<PhysicsAdmissionWait>>,
    mut registry: ResMut<ReadinessRegistry>,
    mut commands: Commands,
) {
    let waiting = !awaiting_dynamic_admission.is_empty()
        || !still_pending.is_empty()
        || body_admission_hold
            .as_deref()
            .is_some_and(|holds| holds.holds(PhysicsHolds::BODY_ADMISSION));
    match (waiting, wait) {
        (true, None) => {
            let awaiting = awaiting_dynamic_admission
                .iter()
                .map(|path| path.path.as_str())
                .take(16)
                .collect::<Vec<_>>();
            info!(
                "[readiness] physics admission held: body_admission_pending={:?}, state_pending={:?}, initialization_pending={:?}, constraint_admission_hold={}",
                awaiting,
                physics_state_pending
                    .iter()
                    .take(16)
                    .map(|path| path.path.as_str())
                    .collect::<Vec<_>>(),
                initialization_pending
                    .iter()
                    .take(16)
                    .map(|path| path.path.as_str())
                    .collect::<Vec<_>>(),
                body_admission_hold
                    .as_deref()
                    .is_some_and(|holds| holds.holds(PhysicsHolds::BODY_ADMISSION)),
            );
            let ticket = registry.begin(
                Subject::World,
                kinds::PARTICIPANT_INIT,
                "USD physics admission",
            );
            commands.insert_resource(PhysicsAdmissionWait { ticket });
        }
        (false, Some(wait)) => {
            info!("[readiness] physics admission complete");
            registry.finish(wait.ticket);
            commands.remove_resource::<PhysicsAdmissionWait>();
        }
        _ => {}
    }
}

/// Find the physical entity that owns a USD-authored program.
///
/// USD program prims are ordinary children of the object they control. The
/// first ancestor carrying `PhysicsRigidBodyAPI` is therefore the authoritative
/// readiness boundary: freezing its subtree also freezes jointed child bodies
/// and colliders, while leaving unrelated scene objects live. This follows the
/// composed hierarchy and schema components rather than guessing from a path or
/// a model name.
fn owning_physics_entity(
    entity: Entity,
    parents: &Query<&ChildOf>,
    rigid_bodies: &Query<(), With<avian3d::prelude::RigidBody>>,
) -> Entity {
    let mut current = entity;
    for _ in 0..64 {
        if rigid_bodies.contains(current) {
            return current;
        }
        let Ok(child_of) = parents.get(current) else {
            break;
        };
        current = child_of.parent();
    }
    entity
}

/// Whether this Modelica participant has a valid initialized solver snapshot.
///
/// Source compilation constructs and samples the live solver at t=0. The
/// active participant is ready when that initial output snapshot is committed;
/// requiring a positive model time would advance the simulation during scene
/// admission. A paused model remains intentionally ready after compilation.
/// A component error remains a named readiness fact, even when the model field
/// has not received the same diagnostic yet.
fn modelica_wait_kind(
    model: &ModelicaModel,
    variables: Option<&ModelicaVariables>,
    component: Option<&SimComponent>,
) -> Option<&'static str> {
    if variables.is_none()
        || model.last_error.is_some()
        || component.is_some_and(|component| {
            matches!(component.status, lunco_cosim_core::SimStatus::Error(_))
        })
    {
        Some(kinds::PROGRAM_FAILED)
    } else if model.is_compiling || !model.is_compiled {
        Some(kinds::PROGRAM_COMPILE)
    } else if !model.paused
        && (!model.current_time.is_finite()
            || model.current_time < 0.0
            || variables.is_some_and(|variables| {
                variables.values.is_empty()
                    || variables.values.values().any(|value| !value.is_finite())
            }))
    {
        Some(kinds::PARTICIPANT_INIT)
    } else {
        None
    }
}

/// Registers the USD scene's readiness producers.
pub struct UsdReadinessPlugin;

impl Plugin for UsdReadinessPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<lunco_readiness::ReadinessPlugin>() {
            app.add_plugins(lunco_readiness::ReadinessPlugin);
        }
        // `PostUpdate`: after the frame's spawning and compile-wrapping have run,
        // so a wait that closed this frame is not re-declared before the state
        // that closes it is visible.
        app.add_observer(finish_program_readiness_wait).add_systems(
            PostUpdate,
            (
                track_scene_load,
                track_program_readiness,
                track_physics_admission,
            ),
        );
        // Every wait belongs to the scene that declared it. The registry already
        // drops waits whose subject entity was despawned, but a WORLD-scoped one
        // has no entity to die with — the outgoing scene's load wait would be
        // inherited by the incoming scene and go on holding physics against it.
        // The producers re-declare from live state on the next frame, so
        // clearing here costs nothing that is still true.
        app.add_systems(
            lunco_core::SceneTeardown,
            |mut registry: ResMut<ReadinessRegistry>,
             mut dirty: ResMut<crate::BindingEpochDirty>,
             wait: Option<Res<crate::BindingEpochWait>>,
             physics_wait: Option<Res<PhysicsAdmissionWait>>,
             mut commands: Commands| {
                registry.clear();
                dirty.0 = true;
                if let Some(w) = wait {
                    registry.finish(w.0);
                    commands.remove_resource::<crate::BindingEpochWait>();
                }
                commands.remove_resource::<SceneLoadWait>();
                if let Some(wait) = physics_wait {
                    registry.finish(wait.ticket);
                    commands.remove_resource::<PhysicsAdmissionWait>();
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn early_port_projection_does_not_release_compile_hold() {
        let compiling = SimComponent {
            status: lunco_cosim_core::SimStatus::Compiling,
            ..default()
        };
        let model = ModelicaModel::default();
        assert_eq!(
            modelica_wait_kind(&model, Some(&ModelicaVariables::default()), None),
            Some(kinds::PROGRAM_COMPILE)
        );
        assert_eq!(
            modelica_wait_kind(
                &model,
                Some(&ModelicaVariables::default()),
                Some(&compiling)
            ),
            Some(kinds::PROGRAM_COMPILE)
        );

        let failed = SimComponent {
            status: lunco_cosim_core::SimStatus::Error("bad model".into()),
            ..default()
        };
        assert_eq!(
            modelica_wait_kind(&model, Some(&ModelicaVariables::default()), Some(&failed)),
            Some(kinds::PROGRAM_FAILED),
            "a failed model remains a named policy fact"
        );

        let compiled = ModelicaModel {
            is_compiled: true,
            paused: true,
            ..default()
        };
        assert_eq!(
            modelica_wait_kind(
                &compiled,
                Some(&ModelicaVariables::default()),
                Some(&compiling)
            ),
            None,
            "a deliberately paused model is ready after its source compiles"
        );

        let active = ModelicaModel {
            is_compiled: true,
            paused: false,
            ..default()
        };
        assert_eq!(
            modelica_wait_kind(
                &active,
                Some(&ModelicaVariables::default()),
                Some(&compiling)
            ),
            Some(kinds::PARTICIPANT_INIT),
            "an active participant remains pending until its initialized t=0 snapshot"
        );

        let initialized = ModelicaModel {
            is_compiled: true,
            paused: false,
            ..default()
        };
        assert_eq!(
            modelica_wait_kind(
                &initialized,
                Some(&ModelicaVariables {
                    values: [("initial_output".to_owned(), 1.0)].into()
                }),
                None
            ),
            None,
            "the committed t=0 solver snapshot closes participant initialization"
        );

        let positive_time_without_snapshot = ModelicaModel {
            is_compiled: true,
            paused: false,
            current_time: active.communication_period_secs,
            ..default()
        };
        assert_eq!(
            modelica_wait_kind(
                &positive_time_without_snapshot,
                Some(&ModelicaVariables::default()),
                None
            ),
            Some(kinds::PARTICIPANT_INIT),
            "a clock value without an output snapshot is not initialized state"
        );

        let invalid_time = ModelicaModel {
            is_compiled: true,
            paused: false,
            current_time: f64::NAN,
            ..default()
        };
        assert_eq!(
            modelica_wait_kind(
                &invalid_time,
                Some(&ModelicaVariables {
                    values: [("initial_output".to_owned(), 1.0)].into()
                }),
                None
            ),
            Some(kinds::PARTICIPANT_INIT),
            "a non-finite solver clock is not evidence of an initial exchange"
        );

        let invalid_snapshot = ModelicaModel {
            is_compiled: true,
            paused: false,
            current_time: 0.0,
            ..default()
        };
        assert_eq!(
            modelica_wait_kind(
                &invalid_snapshot,
                Some(&ModelicaVariables {
                    values: [("initial_output".to_owned(), f64::NAN)].into()
                }),
                None
            ),
            Some(kinds::PARTICIPANT_INIT),
            "non-finite initial observables cannot release participant readiness"
        );
    }

    #[test]
    fn scene_wait_covers_only_stage_mounting() {
        assert!(scene_still_loading(true));
        assert!(!scene_still_loading(false));
    }

    #[test]
    fn physics_admission_wait_covers_body_and_constraint_boundaries() {
        let mut app = App::new();
        app.init_resource::<ReadinessRegistry>()
            .init_resource::<PhysicsHolds>()
            .add_systems(Update, track_physics_admission);

        let body = app
            .world_mut()
            .spawn((ShouldBeDynamic, UsdPrimPath::default()))
            .id();
        app.update();
        let item = app
            .world()
            .resource::<ReadinessRegistry>()
            .pending()
            .next()
            .expect("a held USD body must publish participant readiness");
        assert_eq!(item.subject, Subject::World);
        assert_eq!(item.kind, kinds::PARTICIPANT_INIT);

        app.world_mut().entity_mut(body).remove::<ShouldBeDynamic>();
        app.world_mut()
            .resource_mut::<PhysicsHolds>()
            .set(PhysicsHolds::BODY_ADMISSION, true);
        app.update();
        assert!(
            app.world()
                .resource::<ReadinessRegistry>()
                .pending()
                .next()
                .is_some(),
            "pending joint admission must keep readiness open after body promotion"
        );

        app.world_mut()
            .resource_mut::<PhysicsHolds>()
            .set(PhysicsHolds::BODY_ADMISSION, false);
        app.update();
        assert!(
            app.world()
                .resource::<ReadinessRegistry>()
                .pending()
                .next()
                .is_none(),
            "the participant wait must close only after admission is visible"
        );
    }

    #[test]
    fn pending_model_compile_does_not_hold_the_world() {
        let mut app = App::new();
        app.init_resource::<ReadinessRegistry>()
            .add_systems(Update, track_program_readiness);

        let entity = app
            .world_mut()
            .spawn((
                UsdSourcedCosim,
                ModelicaModel {
                    model_name: "ColdController".into(),
                    ..default()
                },
            ))
            .id();

        app.update();

        let item = app
            .world()
            .resource::<ReadinessRegistry>()
            .pending()
            .next()
            .expect("a compiling model must be tracked");
        assert_eq!(item.subject, Subject::Entity(entity));
        assert_eq!(item.kind, kinds::PROGRAM_COMPILE);
    }

    #[test]
    fn model_compile_wait_targets_the_authored_rigid_body_owner() {
        let mut app = App::new();
        app.init_resource::<ReadinessRegistry>()
            .add_systems(Update, track_program_readiness);

        let body = app
            .world_mut()
            .spawn(avian3d::prelude::RigidBody::Dynamic)
            .id();
        app.world_mut()
            .spawn((ChildOf(body), UsdSourcedCosim, ModelicaModel::default()));

        app.update();

        let item = app
            .world()
            .resource::<ReadinessRegistry>()
            .pending()
            .next()
            .expect("the child model must be tracked");
        assert_eq!(item.subject, Subject::Entity(body));
        assert_eq!(item.action, lunco_readiness::Action::HoldEntity);
    }
}
