//! Declaring what a loading scene is still waiting on.
//!
//! [`lunco_readiness`] holds the registry and the policy; `lunco_physics` does
//! the freezing. This module is the **producer** for the two waits a USD scene
//! actually has, and it is deliberately the only place that knows how to detect
//! them:
//!
//! | Wait | Open while | Scope |
//! |---|---|---|
//! | [`kinds::SCENE_LOAD`] | the stage or its projected participants are still settling | world |
//! | [`kinds::PROGRAM_COMPILE`] | an entity's Modelica model has not compiled | its owning physical subtree |
//!
//! # Why reconcile systems rather than events
//!
//! A ticket opened on an event has to be closed on the matching event, on every
//! path — including the ones that end in a failed asset load, a scene reload
//! halfway through a compile, or a despawn. Missing one leaks a hold that freezes
//! the world until the deadline.
//!
//! These systems instead derive the wait from state that is *already* the truth:
//! `SceneLoadInFlight` and unresolved `UsdAwaitingStage` prims for the scene, a
//! `ModelicaModel` whose interface has not compiled for a model. There is no
//! path to miss, because there is no transition being watched — each frame the
//! wait either still describes the world or it does not.

use bevy::prelude::*;
use lunco_modelica::ModelicaModel;
use lunco_readiness::{kinds, ReadinessRegistry, ReadinessTicket, Subject};

use crate::cosim::{SceneLoadInFlight, UsdSourcedCosim};
use lunco_cosim::SimComponent;
use lunco_usd_bevy::UsdAwaitingStage;

/// The open world-scoped scene-load wait, if a scene is loading.
#[derive(Resource)]
struct SceneLoadWait {
    ticket: ReadinessTicket,
}

/// The open compile wait for this entity's Modelica model.
///
/// On the entity rather than in a side table so it dies with the entity; the
/// registry drops waits whose subject was despawned, so a scene reload
/// mid-compile needs no teardown of its own.
#[derive(Component)]
struct ModelCompileWait {
    ticket: ReadinessTicket,
    kind: &'static str,
    /// The physical owner held while the program is unavailable.  Modelica
    /// programs are commonly authored below a rigid body, so the program
    /// entity itself is not the thing whose initial state must remain fixed.
    owner: Entity,
}

/// Hold the world while a scene and its projected participants are settling.
///
/// The stage signals cover the window before and during prim creation. Once this
/// scene has opened a wait, the co-simulation binding epoch extends that same
/// transaction through asynchronous source loading, Modelica compilation,
/// native joint/wheel admission, and connection settlement. A briefly empty
/// readiness registry is therefore not interpreted as completion before those
/// downstream producers have had a chance to declare themselves.
///
/// [`lunco_cosim::BindingRevision::sealed`] is the explicit projection boundary:
/// it is opened by endpoint lifecycle changes and sealed only by
/// `settle_binding_epoch` after every projected participant is terminal. The
/// existing scene ticket owns the whole interval; this does not create a second
/// loading protocol or infer readiness from elapsed frames.
fn track_scene_load(
    in_flight: Option<Res<SceneLoadInFlight>>,
    awaiting: Query<(), With<UsdAwaitingStage>>,
    binding: Res<lunco_cosim::BindingRevision>,
    wait: Option<Res<SceneLoadWait>>,
    mut registry: ResMut<ReadinessRegistry>,
    mut commands: Commands,
) {
    let stage_loading = in_flight.is_some() || !awaiting.is_empty();
    let loading = scene_still_loading(stage_loading, wait.is_some(), binding.sealed);
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

fn scene_still_loading(stage_loading: bool, wait_open: bool, binding_sealed: bool) -> bool {
    // `sealed` defaults false before any scene exists. Only an already-open
    // scene wait may extend itself through the projection epoch, so an empty
    // application does not manufacture a load transaction.
    stage_loading || (wait_open && !binding_sealed)
}

/// Freeze the physical owner of a Modelica entity whose program has not compiled
/// yet, and release its wait the moment that program is runnable (or terminally
/// failed).
///
/// This is the descent-lander race, closed: the entity exists and has mass and a
/// collider long before the model that is supposed to fly it has been through the
/// compiler. Until it has, the object is not a vehicle — it is a rock with a
/// pending appointment — and it must not be falling.
///
/// The `ModelicaModel` lifecycle is authoritative here. A bind-published
/// [`SimComponent`] intentionally remains `Compiling` until its first solver
/// tick, but that first tick cannot happen while offline recording is waiting
/// for the visual gate. Waiting on the component status would therefore make
/// the recorder wait on the event that the recorder itself must initiate.
///
/// The wait is deliberately attached to the owning physical subtree rather than
/// the whole world. A cold compiler must not stop unrelated rovers, terrain
/// streaming, or already-ready physics. The owner is derived from the authored
/// entity hierarchy and the runtime rigid-body schema; there is no vehicle-name
/// or program-name special case. A standalone program with no rigid-body
/// ancestor holds its own entity, preserving the original participant scope.
fn track_model_compiles(
    models: Query<(Entity, &ModelicaModel, Option<&SimComponent>), With<UsdSourcedCosim>>,
    waits: Query<(Entity, &ModelCompileWait)>,
    parents: Query<&ChildOf>,
    rigid_bodies: Query<(), With<avian3d::prelude::RigidBody>>,
    mut registry: ResMut<ReadinessRegistry>,
    mut commands: Commands,
) {
    for (entity, model, component) in &models {
        let kind = modelica_wait_kind(model, component);
        let wait = waits.get(entity).ok();
        let owner = owning_physics_entity(entity, &parents, &rigid_bodies);
        match (kind, wait) {
            (Some(kind), None) => {
                let ticket = registry.begin(Subject::Entity(owner), kind, model.model_name.clone());
                commands.entity(entity).try_insert(ModelCompileWait {
                    ticket,
                    kind,
                    owner,
                });
            }
            (None, Some((_, wait))) => {
                registry.finish(wait.ticket);
                commands.entity(entity).try_remove::<ModelCompileWait>();
            }
            (Some(kind), Some((_, wait))) if kind != wait.kind || owner != wait.owner => {
                registry.finish(wait.ticket);
                let ticket = registry.begin(Subject::Entity(owner), kind, model.model_name.clone());
                commands.entity(entity).try_insert(ModelCompileWait {
                    ticket,
                    kind,
                    owner,
                });
            }
            _ => {}
        }
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

/// Whether the simulation must remain frozen for this model's compiler.
///
/// Source compilation and the first live solver tick are different lifecycle
/// events. `ModelicaModel.is_compiled` closes this wait; the Modelica sync loop
/// then performs the first tick and promotes the public component to Running.
/// A component error remains a named readiness fact, even when the model field
/// has not received the same diagnostic yet.
fn modelica_wait_kind(
    model: &ModelicaModel,
    component: Option<&SimComponent>,
) -> Option<&'static str> {
    if model.last_error.is_some()
        || component
            .is_some_and(|component| matches!(component.status, lunco_cosim::SimStatus::Error(_)))
    {
        Some(kinds::PROGRAM_FAILED)
    } else if model.is_compiling || !model.is_compiled {
        Some(kinds::PROGRAM_COMPILE)
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
        app.add_systems(PostUpdate, (track_scene_load, track_model_compiles));
        // Every wait belongs to the scene that declared it. The registry already
        // drops waits whose subject entity was despawned, but a WORLD-scoped one
        // has no entity to die with — the outgoing scene's load wait would be
        // inherited by the incoming scene and go on holding physics against it.
        // Both producers above re-declare from live state on the next frame, so
        // clearing here costs nothing that is still true.
        app.add_systems(
            lunco_usd_bevy::scene_lifecycle::SceneTeardown,
            |mut registry: ResMut<ReadinessRegistry>,
             mut dirty: ResMut<crate::cosim::BindingEpochDirty>,
             wait: Option<Res<crate::cosim::BindingEpochWait>>,
             mut commands: Commands| {
                registry.clear();
                dirty.0 = true;
                if let Some(w) = wait {
                    registry.finish(w.0);
                    commands.remove_resource::<crate::cosim::BindingEpochWait>();
                }
                commands.remove_resource::<SceneLoadWait>();
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
            status: lunco_cosim::SimStatus::Compiling,
            ..default()
        };
        let model = ModelicaModel::default();
        assert_eq!(
            modelica_wait_kind(&model, None),
            Some(kinds::PROGRAM_COMPILE)
        );
        assert_eq!(
            modelica_wait_kind(&model, Some(&compiling)),
            Some(kinds::PROGRAM_COMPILE)
        );

        let failed = SimComponent {
            status: lunco_cosim::SimStatus::Error("bad model".into()),
            ..default()
        };
        assert_eq!(
            modelica_wait_kind(&model, Some(&failed)),
            Some(kinds::PROGRAM_FAILED),
            "a failed model remains a named policy fact"
        );

        let compiled = ModelicaModel {
            is_compiled: true,
            ..default()
        };
        assert_eq!(
            modelica_wait_kind(&compiled, Some(&compiling)),
            None,
            "the first solver tick must not be blocked by the component's bind-time status"
        );
    }

    #[test]
    fn scene_wait_spans_stage_and_participant_projection_transactions() {
        assert!(scene_still_loading(true, false, false));
        assert!(scene_still_loading(true, true, false));
        assert!(
            scene_still_loading(false, true, false),
            "a drained stage is not ready while its binding epoch remains open"
        );
        assert!(!scene_still_loading(false, true, true));
        assert!(
            !scene_still_loading(false, false, false),
            "an empty app must not synthesize a scene transaction"
        );
    }

    #[test]
    fn pending_model_compile_does_not_hold_the_world() {
        let mut app = App::new();
        app.init_resource::<ReadinessRegistry>()
            .add_systems(Update, track_model_compiles);

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
            .add_systems(Update, track_model_compiles);

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
