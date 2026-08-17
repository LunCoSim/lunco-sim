//! The shared scene teardown schedule.
//!
//! A scene owns entities and non-entity state. Entity despawns are driven by the
//! scene command owner; resources, caches, and worker handles are reset here by
//! the subsystem that owns them. Keeping this boundary in `lunco-core` makes it
//! available to every domain without coupling a simulation domain to the USD
//! projection crate.

use bevy::ecs::schedule::ScheduleLabel;
use bevy::prelude::*;
use std::collections::HashSet;

/// Runs when a scene is unloaded, before the replacement scene integrates.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct SceneTeardown;

/// The scene roots that are currently allowed to complete USD projection.
///
/// Scene replacement is intentionally a transaction boundary.  Despawns are
/// deferred by Bevy, so an outgoing root can still be visible to a projection
/// query for part of the frame in which a new load was requested.  Keeping the
/// ownership decision here lets projection systems reject those stale entities
/// before they enqueue visual, camera, or physics commands against them.
///
/// The set, rather than only one "active" root, is deliberate: `OpenFile` is
/// an additive mount and may coexist with the primary running scene.  A normal
/// `LoadScene`, `RestartScene`, or `ClearScene` invalidates the complete set.
#[derive(Resource, Debug, Default)]
pub struct SceneMountState {
    roots: HashSet<Entity>,
    active_root: Option<Entity>,
}

impl SceneMountState {
    /// Invalidate every currently mounted scene before deferred teardown.
    pub fn begin_replacement(&mut self) {
        self.roots.clear();
        self.active_root = None;
    }

    /// Register a root after its entity has been spawned.  `primary` is true
    /// for the normal running scene and false for an additive import.
    pub fn register_root(&mut self, root: Entity, primary: bool) {
        self.roots.insert(root);
        if primary {
            self.active_root = Some(root);
        }
    }

    /// Whether a scene root is still owned by the current mount transaction.
    pub fn contains_root(&self, root: Entity) -> bool {
        self.roots.contains(&root)
    }

    /// The primary running scene root, if one has been mounted.
    pub fn active_root(&self) -> Option<Entity> {
        self.active_root
    }
}

/// Run [`SceneTeardown`] at the scene replacement boundary.
pub fn run_scene_teardown(world: &mut World) {
    if world.try_run_schedule(SceneTeardown).is_err() {
        debug!("[scene] no SceneTeardown schedule registered");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Clone, PartialEq, Debug)]
    struct SceneOnly(u32);

    #[derive(Resource, Clone, PartialEq, Debug)]
    struct AppOwned(u32);

    #[test]
    fn teardown_removes_scene_state_and_restores_app_state() {
        let mut app = App::new();
        app.add_systems(SceneTeardown, |mut commands: Commands| {
            commands.remove_resource::<SceneOnly>();
            commands.insert_resource(AppOwned(1));
        });

        app.insert_resource(SceneOnly(42));
        app.insert_resource(AppOwned(99));
        run_scene_teardown(app.world_mut());

        assert!(app.world().get_resource::<SceneOnly>().is_none());
        assert_eq!(app.world().get_resource::<AppOwned>(), Some(&AppOwned(1)));
    }

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

    #[test]
    fn missing_schedule_is_not_a_panic() {
        let mut app = App::new();
        run_scene_teardown(app.world_mut());
    }

    #[test]
    fn replacement_invalidates_primary_and_additive_roots() {
        let mut state = SceneMountState::default();
        let primary = Entity::from_raw(1);
        let additive = Entity::from_raw(2);
        state.register_root(primary, true);
        state.register_root(additive, false);
        assert!(state.contains_root(primary));
        assert!(state.contains_root(additive));
        assert_eq!(state.active_root(), Some(primary));

        state.begin_replacement();
        assert!(!state.contains_root(primary));
        assert!(!state.contains_root(additive));
        assert_eq!(state.active_root(), None);
    }
}
