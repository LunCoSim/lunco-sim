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

/// Resource resets run after domain shutdown, so terminal callbacks cannot
/// repopulate a store that has already been cleared.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct SceneResourceTeardown;

/// State whose scene lifetime is installed together with its initialization.
///
/// The default reset drops all owned values, including task handles. Mixed
/// application/scene stores override this method to retain application state.
pub trait SceneResource: Resource<Mutability = bevy::ecs::component::Mutable> + Default {
    fn clear_scene(&mut self) {
        *self = Self::default();
    }
}

impl<M: Message> SceneResource for Messages<M> {
    fn clear_scene(&mut self) {
        self.clear();
    }
}

/// Register scene state at its owner, without a separate cleanup registration.
pub trait AppSceneExt {
    fn init_scene_resource<R: SceneResource>(&mut self) -> &mut Self;
}

struct SceneResourcePlugin<R>(std::marker::PhantomData<R>);

impl<R: SceneResource> Plugin for SceneResourcePlugin<R> {
    fn build(&self, app: &mut App) {
        app.init_resource::<R>()
            .add_systems(SceneResourceTeardown, |mut resource: ResMut<R>| {
                resource.clear_scene()
            });
    }
}

impl AppSceneExt for App {
    fn init_scene_resource<R: SceneResource>(&mut self) -> &mut Self {
        if !self.is_plugin_added::<SceneResourcePlugin<R>>() {
            self.add_plugins(SceneResourcePlugin::<R>(std::marker::PhantomData));
        }
        self
    }
}

/// Scene-owned entities outside a scene's structural hierarchy.
/// Children are reclaimed through Bevy's normal linked hierarchy despawn.
#[derive(Component, Debug, Default)]
pub struct SceneOwned;

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
    run_scene_teardown_with_entities(world, []);
}

/// Retire the scene hierarchy inside the same boundary as its domain state.
/// Entity-removal observers and their deferred work finish before resource resets.
pub fn run_scene_teardown_with_entities(
    world: &mut World,
    entities: impl IntoIterator<Item = Entity>,
) {
    if world.try_run_schedule(SceneTeardown).is_err() {
        debug!("[scene] no SceneTeardown schedule registered");
    }
    for entity in entities {
        if let Ok(entity) = world.get_entity_mut(entity) {
            entity.despawn();
        }
    }
    world.flush();
    let entities: Vec<_> = world
        .query_filtered::<Entity, With<SceneOwned>>()
        .iter(world)
        .collect();
    for entity in entities {
        if let Ok(entity) = world.get_entity_mut(entity) {
            entity.despawn();
        }
    }
    world.flush();
    let _ = world.try_run_schedule(SceneResourceTeardown);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Clone, PartialEq, Debug)]
    struct SceneOnly(u32);

    #[derive(Resource, Clone, PartialEq, Debug)]
    struct AppOwned(u32);

    #[derive(Component)]
    struct PublishesOnRemoval;

    #[derive(Resource, Default)]
    struct RegisteredState {
        values: Vec<u32>,
        resets: u32,
    }

    impl SceneResource for RegisteredState {
        fn clear_scene(&mut self) {
            self.values.clear();
            self.resets += 1;
        }
    }

    #[test]
    fn scene_lifecycle_registration_owns_repeatable_cleanup() {
        let mut app = App::new();
        app.init_scene_resource::<RegisteredState>()
            .init_scene_resource::<RegisteredState>()
            .insert_resource(AppOwned(99))
            .add_systems(SceneTeardown, |mut state: ResMut<RegisteredState>| {
                state.values.push(100);
            })
            .add_observer(
                |_: On<Remove, PublishesOnRemoval>, mut commands: Commands| {
                    commands.queue(|world: &mut World| {
                        world.resource_mut::<RegisteredState>().values.push(200);
                    });
                },
            );
        for cycle in 1..=3 {
            app.world_mut()
                .resource_mut::<RegisteredState>()
                .values
                .push(cycle);
            let root = app.world_mut().spawn(SceneOwned).id();
            let child = app.world_mut().spawn(ChildOf(root)).id();
            let survivor = app.world_mut().spawn_empty().id();
            let structural_root = app.world_mut().spawn(PublishesOnRemoval).id();
            run_scene_teardown_with_entities(app.world_mut(), [structural_root]);
            let state = app.world().resource::<RegisteredState>();
            assert!(state.values.is_empty());
            assert_eq!(state.resets, cycle, "registration is idempotent");
            assert!(app.world().get_entity(root).is_err());
            assert!(app.world().get_entity(child).is_err());
            assert!(app.world().get_entity(structural_root).is_err());
            assert!(app.world().get_entity(survivor).is_ok());
            assert_eq!(app.world().resource::<AppOwned>().0, 99);
        }
    }

    #[derive(Resource, Default)]
    struct DefaultSceneState(Vec<u32>);

    impl SceneResource for DefaultSceneState {}

    #[test]
    fn scene_lifecycle_default_reset_drops_owned_values() {
        let mut app = App::new();
        app.init_scene_resource::<DefaultSceneState>();
        app.world_mut()
            .resource_mut::<DefaultSceneState>()
            .0
            .push(42);
        run_scene_teardown(app.world_mut());
        assert!(app.world().resource::<DefaultSceneState>().0.is_empty());
    }

    #[derive(Message)]
    struct SceneNotice(u32);

    #[derive(Message)]
    struct ApplicationNotice;

    #[test]
    fn scene_message_queue_retires_after_shutdown_and_removal_publication() {
        let mut app = App::new();
        app.add_message::<SceneNotice>()
            .init_scene_resource::<Messages<SceneNotice>>()
            .add_message::<ApplicationNotice>()
            .add_systems(SceneTeardown, |mut notices: MessageWriter<SceneNotice>| {
                notices.write(SceneNotice(100));
            })
            .add_observer(
                |_: On<Remove, PublishesOnRemoval>, mut notices: MessageWriter<SceneNotice>| {
                    notices.write(SceneNotice(200));
                },
            );
        app.world_mut().write_message(ApplicationNotice);
        let mut cursor = bevy::ecs::message::MessageCursor::<SceneNotice>::default();
        for cycle in 1..=3 {
            app.world_mut().write_message(SceneNotice(0));
            let outgoing = app.world_mut().spawn(PublishesOnRemoval).id();
            run_scene_teardown_with_entities(app.world_mut(), [outgoing]);
            let messages = app.world().resource::<Messages<SceneNotice>>();
            assert!(messages.is_empty());
            assert_eq!(
                app.world().resource::<Messages<ApplicationNotice>>().len(),
                1
            );

            app.world_mut().write_message(SceneNotice(cycle));
            let messages = app.world().resource::<Messages<SceneNotice>>();
            assert_eq!(
                cursor
                    .read(messages)
                    .map(|notice| notice.0)
                    .collect::<Vec<_>>(),
                [cycle]
            );
        }
    }

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
        let primary = Entity::from_raw_u32(1).expect("test entity index is valid");
        let additive = Entity::from_raw_u32(2).expect("test entity index is valid");
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
