//! [`SceneTeardown`] — the schedule that unloads a scene's non-entity state.
//!
//! # The invariant
//!
//! A scene owns more than its entities. Anything a scene load writes — a
//! resource, a cache, a worker-side handle — belongs to that scene and must not
//! be visible to the next one. Unloading despawns the entities; this schedule
//! unloads everything else, so loading scene A then scene B leaves no value A
//! chose in force. Without it the failure is silent: nothing errors, the scene
//! simply behaves as though it were still the previous one.
//!
//! The entity side states the same rule structurally — the celestial subsystem
//! tags what it spawns `CelestialDerived` and teardown despawns that set, rather
//! than enumerating what a reload ought to remove.
//!
//! # It is a schedule, not a registry
//!
//! Bevy already expresses "run these systems when this lifecycle edge happens":
//! that is `OnExit`. Scene load here is a command rather than a state
//! transition, so this is the same idea under an explicit label — teardown runs
//! [`SceneTeardown`], and any crate that derives state from a scene adds its own
//! reset system to it.
//!
//! The ownership direction is the point. A central registry puts every
//! subsystem's cleanup in one file that no subsystem author edits, and the state
//! that gets forgotten is the one whose owner never looked there. With a
//! schedule the reset lives beside the code that writes the state, and
//! `SceneTeardown` grep-lists everything a reload restores.
//!
//! ```ignore
//! app.add_systems(SceneTeardown, |mut commands: Commands| {
//!     commands.remove_resource::<MySceneCache>();
//! });
//! ```
//!
//! # Removing versus restoring
//!
//! Both are ordinary systems, and which one is right depends on who owns the
//! value. State that only means something while a scene is loaded — caches,
//! provenance records — is REMOVED; absence is its correct empty state. State the
//! app installs at start-up and a scene merely overrides — gravity is the type
//! case — is RESTORED to the app's baseline, because removing it would leave the
//! world with no value at all.

use bevy::ecs::schedule::ScheduleLabel;
use bevy::prelude::*;

/// Runs when a scene is unloaded, before the next one loads.
///
/// Add a system here for any resource, cache or non-entity state derived from
/// the loaded scene. Entities are handled separately, by despawn.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct SceneTeardown;

/// Run [`SceneTeardown`], as an exclusive-world command.
///
/// Exclusive so it lands at a deferred flush point, in step with the entity
/// despawns of the same teardown: state cleared while systems still hold the old
/// scene's entities would be read back before the world is consistent.
///
/// `add_systems` creates the schedule on first use, so it exists as soon as any
/// crate registers a reset. The fallible run covers the case where none has:
/// an app with nothing to restore tears its entities down and moves on.
pub fn run_scene_teardown(world: &mut World) {
    if world.try_run_schedule(SceneTeardown).is_err() {
        debug!("[clear-scene] no SceneTeardown schedule registered");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Clone, PartialEq, Debug)]
    struct SceneOnly(u32);

    #[derive(Resource, Clone, PartialEq, Debug)]
    struct AppOwned(u32);

    /// The leak this exists to prevent: a value one scene wrote must not be
    /// visible to the next. Both dispositions in one test, because the whole
    /// point is that a subsystem picks the right one.
    #[test]
    fn teardown_removes_scene_state_and_restores_app_state() {
        let mut app = App::new();
        app.add_systems(SceneTeardown, |mut commands: Commands| {
            commands.remove_resource::<SceneOnly>();
            commands.insert_resource(AppOwned(1));
        });

        // A scene loads and writes both.
        app.insert_resource(SceneOnly(42));
        app.insert_resource(AppOwned(99));

        run_scene_teardown(app.world_mut());

        assert!(
            app.world().get_resource::<SceneOnly>().is_none(),
            "scene-only resource must not survive its scene"
        );
        assert_eq!(
            app.world().get_resource::<AppOwned>(),
            Some(&AppOwned(1)),
            "app-owned resource must be restored to its baseline, not removed"
        );
    }

    /// Teardown must be repeatable — a reload is not a one-shot, and a mechanism
    /// that disarms after the first scene swap would fail exactly where it is
    /// needed most.
    #[test]
    fn teardown_is_repeatable() {
        let mut app = App::new();
        app.add_systems(SceneTeardown, |mut commands: Commands| {
            commands.remove_resource::<SceneOnly>();
        });

        for value in [1u32, 2, 3] {
            app.insert_resource(SceneOnly(value));
            run_scene_teardown(app.world_mut());
            assert!(app.world().get_resource::<SceneOnly>().is_none());
        }
    }

    /// An app that never initialised the schedule still tears its entities down.
    #[test]
    fn missing_schedule_is_not_a_panic() {
        let mut app = App::new();
        run_scene_teardown(app.world_mut());
    }
}
