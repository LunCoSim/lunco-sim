//! Declaring what a loading scene is still waiting on.
//!
//! [`lunco_readiness`] holds the registry and the policy; `lunco_physics` does
//! the freezing. This module is the **producer** for the two waits a USD scene
//! actually has, and it is deliberately the only place that knows how to detect
//! them:
//!
//! | Wait | Open while | Scope |
//! |---|---|---|
//! | [`kinds::SCENE_LOAD`] | the stage is still spawning prims | world |
//! | [`kinds::PROGRAM_COMPILE`] | an entity's Modelica model has not compiled | that entity |
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
//! `ModelicaModel` without its `SimComponent` for a model. There is no path to
//! miss, because there is no transition being watched — each frame the wait
//! either still describes the world or it does not.

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

/// The open per-entity compile wait for this entity's Modelica model.
///
/// On the entity rather than in a side table so it dies with the entity; the
/// registry drops waits whose subject was despawned, so a scene reload
/// mid-compile needs no teardown of its own.
#[derive(Component)]
struct ModelCompileWait {
    ticket: ReadinessTicket,
}

/// Hold the world while a scene is spawning, and release when it has finished.
///
/// Both signals are needed and neither subsumes the other, for the same reason
/// the status-bar mirror needs both: `SceneLoadInFlight` covers the window
/// *before any prim entity exists* (which an entity count reads as "nothing to
/// wait for"), and leftover `UsdAwaitingStage` prims cover deferred instance and
/// reference spawns that have no `LoadScene` behind them.
fn track_scene_load(
    in_flight: Option<Res<SceneLoadInFlight>>,
    awaiting: Query<(), With<UsdAwaitingStage>>,
    wait: Option<Res<SceneLoadWait>>,
    mut registry: ResMut<ReadinessRegistry>,
    mut commands: Commands,
) {
    let loading = in_flight.is_some() || !awaiting.is_empty();
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

/// Freeze an object whose Modelica model has not compiled yet, and release it
/// the moment the model is live.
///
/// This is the descent-lander race, closed: the entity exists and has mass and a
/// collider long before the model that is supposed to fly it has been through the
/// compiler. Until it has, the object is not a vehicle — it is a rock with a
/// pending appointment — and it must not be falling.
///
/// `SimComponent` is the right "ready" signal rather than the compile callback:
/// it is only inserted once the model has produced its variables, so it means
/// *the model is stepping*, not merely *the compiler returned*.
fn track_model_compiles(
    unready: Query<
        (Entity, &ModelicaModel),
        (
            With<UsdSourcedCosim>,
            Without<SimComponent>,
            Without<ModelCompileWait>,
        ),
    >,
    ready: Query<(Entity, &ModelCompileWait), With<SimComponent>>,
    mut registry: ResMut<ReadinessRegistry>,
    mut commands: Commands,
) {
    for (entity, model) in &unready {
        let ticket = registry.begin(
            Subject::Entity(entity),
            kinds::PROGRAM_COMPILE,
            model.model_name.clone(),
        );
        commands
            .entity(entity)
            .try_insert(ModelCompileWait { ticket });
    }

    for (entity, wait) in &ready {
        registry.finish(wait.ticket);
        commands.entity(entity).try_remove::<ModelCompileWait>();
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
    }
}
