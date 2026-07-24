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
/// A compiling [`SimComponent`] is not ready even though it already exposes its
/// USD-declared ports. The early interface prevents false dangling-wire errors;
/// its status, rather than component existence, keeps the physics hold until a
/// compiler result has made the model runnable or visibly failed.
fn track_model_compiles(
    models: Query<(Entity, &ModelicaModel, Option<&SimComponent>), With<UsdSourcedCosim>>,
    waits: Query<(Entity, &ModelCompileWait)>,
    mut registry: ResMut<ReadinessRegistry>,
    mut commands: Commands,
) {
    for (entity, model, component) in &models {
        let compiling = model_compile_pending(component);
        let wait = waits.get(entity).ok();
        match (compiling, wait) {
            (true, None) => {
                let ticket = registry.begin(
                    Subject::Entity(entity),
                    kinds::PROGRAM_COMPILE,
                    model.model_name.clone(),
                );
                commands
                    .entity(entity)
                    .try_insert(ModelCompileWait { ticket });
            }
            (false, Some((_, wait))) => {
                registry.finish(wait.ticket);
                commands.entity(entity).try_remove::<ModelCompileWait>();
            }
            _ => {}
        }
    }
}

/// Whether the simulation must remain frozen for this model's compiler.
///
/// No component means the source has parsed but its public interface has not
/// been projected yet. `Error` is deliberately terminal rather than pending:
/// the user needs an actionable failure, not an indefinite frozen world.
fn model_compile_pending(component: Option<&SimComponent>) -> bool {
    component.is_none_or(|component| component.status == lunco_cosim::SimStatus::Compiling)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn early_port_projection_does_not_release_compile_hold() {
        let compiling = SimComponent {
            status: lunco_cosim::SimStatus::Compiling,
            ..default()
        };
        assert!(model_compile_pending(None));
        assert!(model_compile_pending(Some(&compiling)));

        let failed = SimComponent {
            status: lunco_cosim::SimStatus::Error("bad model".into()),
            ..default()
        };
        assert!(
            !model_compile_pending(Some(&failed)),
            "a failed model reports its error instead of freezing the world forever"
        );
    }
}
