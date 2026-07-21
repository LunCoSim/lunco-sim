//! Turning readiness decisions into physics.
//!
//! [`lunco_readiness`] answers *what is pending and what should that freeze*.
//! This module is the half that actually freezes it, and it is the only place
//! that knows a decision has anything to do with rigid bodies.
//!
//! Two effects, matching the two scopes of [`Action`](lunco_readiness::Action):
//!
//! - **World** — [`ReadinessState::world_hold`] raises the
//!   [`PhysicsHolds::READINESS`] hold, which pauses `Time<Physics>` exactly like a
//!   terrain bake does. Everything else — the tick, the epoch, scripts — keeps
//!   running, so the script that is waiting on something can still run to see it
//!   arrive.
//! - **Entity** — an entity marked [`HeldForReadiness`] has its rigid bodies and
//!   its colliders disabled, itself and throughout its subtree.
//!
//! # Why both components
//!
//! `RigidBodyDisabled` alone is not enough, and avian says so: *"this component
//! does not disable collision detection or spatial queries for colliders attached
//! to the rigid body."* A body frozen with only that still pushes its neighbours
//! around — an object that is not ready would shove a rover off a ramp while
//! standing perfectly still itself. `ColliderDisabled` is what makes it
//! non-interacting as well as non-moving.
//!
//! # Why the whole subtree
//!
//! A USD vehicle is not one entity. Its colliders are child prims and its wheels
//! are separate bodies joined to the hull. Freezing only the root would leave the
//! wheels simulating against a hull that cannot respond.
//!
//! # Why a reconcile, not an observer
//!
//! The entities that need holding are the ones that were just spawned, and a
//! spawn is not atomic — colliders, joints and child bodies land over the next
//! several frames. An observer on `Add` would run once, before most of the
//! subtree existed, and freeze a fraction of it. The reconcile runs every frame
//! and is idempotent, so a child that appears three frames late is frozen on the
//! frame it appears.
//!
//! # Forces
//!
//! A frozen body must not be *accumulating* force. avian clears velocity
//! increments in `clear_velocity_increments`, which is `With<SolverBody>` — and a
//! disabled body has no `SolverBody`. So force applied to a frozen body is never
//! cleared, sums for the whole hold, and discharges into a single step on
//! release. That is the same failure that launched a rover at 224 m/s through the
//! ground (see [`crate::physics_is_live`], the world-scoped version of this
//! hazard).
//!
//! The rule that prevents it is one line and belongs to every force producer:
//! **apply force only to bodies the solver will integrate.** [`Integrable`] is
//! that filter, and `lunco-environment`, `lunco-cosim` and `lunco-mobility` build
//! their force queries from it.

use avian3d::prelude::*;
use bevy::prelude::*;
use lunco_readiness::{HeldForReadiness, ReadinessSet, ReadinessState};

use crate::PhysicsHolds;

/// Query filter for a rigid body the solver will integrate this tick.
///
/// **Every system that applies force, torque, impulse or acceleration must build
/// its query from this.** A body that is disabled — because it is
/// [held for readiness](HeldForReadiness), or for any other reason — never has
/// its accumulators cleared, so force applied to it is stored rather than spent,
/// and lands in full on the one step that eventually runs.
pub type Integrable = (With<RigidBody>, Without<RigidBodyDisabled>);

/// Marks one entity that this module disabled, and what it disabled — so release
/// restores exactly what was taken and nothing else.
///
/// The precision matters: a body may already be disabled for its own reasons
/// (authored that way, disabled by a script). Recording what *we* inserted means
/// releasing a readiness hold never re-enables something somebody else switched
/// off.
#[derive(Component, Debug, Clone, Copy)]
pub struct FrozenForReadiness {
    /// The [`HeldForReadiness`] entity this freeze belongs to. A subtree is
    /// released as a unit, keyed on its root.
    pub owner: Entity,
    /// This module inserted `RigidBodyDisabled` here.
    body: bool,
    /// This module inserted `ColliderDisabled` here.
    collider: bool,
}

/// Project [`ReadinessState::world_hold`] onto [`PhysicsHolds`].
///
/// Change-guarded, so it writes only on an edge and a readiness hold composes
/// with every other hold rather than fighting them: physics runs when the whole
/// set is empty, and readiness is one member of that set.
pub fn apply_world_readiness_hold(state: Res<ReadinessState>, mut holds: ResMut<PhysicsHolds>) {
    if holds.holds(PhysicsHolds::READINESS) != state.world_hold {
        holds.set(PhysicsHolds::READINESS, state.world_hold);
    }
}

/// Disable bodies and colliders under every [`HeldForReadiness`] root, and
/// re-enable them when the mark clears.
///
/// Idempotent — safe (and intended) to run every frame.
pub fn reconcile_frozen_subtrees(
    held: Query<Entity, With<HeldForReadiness>>,
    children: Query<&Children>,
    bodies: Query<(), (With<RigidBody>, Without<RigidBodyDisabled>)>,
    colliders: Query<(), (With<Collider>, Without<ColliderDisabled>)>,
    frozen: Query<(Entity, &FrozenForReadiness)>,
    mut commands: Commands,
) {
    // ── Freeze: everything under a held root that is not frozen yet ──────────
    for root in &held {
        for entity in std::iter::once(root).chain(children.iter_descendants(root)) {
            let body = bodies.contains(entity);
            let collider = colliders.contains(entity);
            if !body && !collider {
                continue;
            }
            let mut e = commands.entity(entity);
            if body {
                e.try_insert(RigidBodyDisabled);
            }
            if collider {
                e.try_insert(ColliderDisabled);
            }
            // Merge rather than overwrite: on a later frame this entity may gain
            // a collider it did not have when its body was first frozen, and the
            // record must remember both so release undoes both.
            let prior = frozen.get(entity).ok().map(|(_, f)| *f);
            e.try_insert(FrozenForReadiness {
                owner: root,
                body: body || prior.is_some_and(|f| f.body),
                collider: collider || prior.is_some_and(|f| f.collider),
            });
        }
    }

    // ── Release: anything whose owner is no longer held (or is gone) ─────────
    for (entity, record) in &frozen {
        if held.contains(record.owner) {
            continue;
        }
        let mut e = commands.entity(entity);
        if record.body {
            e.try_remove::<RigidBodyDisabled>();
        }
        if record.collider {
            e.try_remove::<ColliderDisabled>();
        }
        e.try_remove::<FrozenForReadiness>();
    }
}

/// Installs the physics side of readiness. Added by [`crate::PhysicsGatePlugin`],
/// so whoever owns physics owns the enforcement of a readiness decision.
pub struct ReadinessEffectPlugin;

impl Plugin for ReadinessEffectPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<lunco_readiness::ReadinessPlugin>() {
            app.add_plugins(lunco_readiness::ReadinessPlugin);
        }
        app.add_systems(
            PreUpdate,
            (apply_world_readiness_hold, reconcile_frozen_subtrees)
                .chain()
                // After the decision they enforce, and before
                // `apply_physics_holds` projects the hold set onto the clock.
                .after(ReadinessSet)
                .before(crate::apply_physics_holds),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_readiness::{kinds, ReadinessRegistry, Subject};

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<PhysicsHolds>()
            .init_resource::<Time<Physics>>()
            .add_plugins(ReadinessEffectPlugin);
        app
    }

    /// A vehicle is a subtree, so freezing must reach the child collider — the
    /// hull would otherwise stand still while its wheels kept simulating.
    #[test]
    fn a_held_subtree_is_disabled_and_restored_as_a_unit() {
        let mut app = app();
        let hull = app
            .world_mut()
            .spawn((RigidBody::Dynamic, Collider::sphere(1.0)))
            .id();
        let wheel = app
            .world_mut()
            .spawn((RigidBody::Dynamic, Collider::sphere(0.3), ChildOf(hull)))
            .id();

        let ticket = app.world_mut().resource_mut::<ReadinessRegistry>().begin(
            Subject::Entity(hull),
            kinds::PROGRAM_COMPILE,
            "guidance",
        );
        app.update();

        for e in [hull, wheel] {
            assert!(
                app.world().entity(e).contains::<RigidBodyDisabled>(),
                "{e} should not move while its program is compiling"
            );
            assert!(
                app.world().entity(e).contains::<ColliderDisabled>(),
                "{e} should not push anything either"
            );
        }

        app.world_mut()
            .resource_mut::<ReadinessRegistry>()
            .finish(ticket);
        app.update();

        for e in [hull, wheel] {
            assert!(!app.world().entity(e).contains::<RigidBodyDisabled>());
            assert!(!app.world().entity(e).contains::<ColliderDisabled>());
            assert!(!app.world().entity(e).contains::<FrozenForReadiness>());
        }
    }

    /// A body somebody else disabled must still be disabled after a readiness
    /// hold comes and goes. Release restores what this module took, not whatever
    /// it happens to find.
    #[test]
    fn release_does_not_re_enable_a_body_disabled_by_someone_else() {
        let mut app = app();
        let e = app
            .world_mut()
            .spawn((RigidBody::Dynamic, Collider::sphere(1.0), RigidBodyDisabled))
            .id();

        let ticket = app.world_mut().resource_mut::<ReadinessRegistry>().begin(
            Subject::Entity(e),
            kinds::PROGRAM_COMPILE,
            "x",
        );
        app.update();
        app.world_mut()
            .resource_mut::<ReadinessRegistry>()
            .finish(ticket);
        app.update();

        assert!(
            app.world().entity(e).contains::<RigidBodyDisabled>(),
            "the pre-existing disable is not ours to lift"
        );
        assert!(
            !app.world().entity(e).contains::<ColliderDisabled>(),
            "the collider disable WAS ours, and is lifted"
        );
    }

    /// Part of the subtree can arrive after the freeze — that is the normal
    /// spawn, not an edge case — and must be frozen when it does.
    #[test]
    fn a_child_spawned_after_the_freeze_is_frozen_too() {
        let mut app = app();
        let hull = app.world_mut().spawn(RigidBody::Dynamic).id();
        app.world_mut().resource_mut::<ReadinessRegistry>().begin(
            Subject::Entity(hull),
            kinds::PROGRAM_COMPILE,
            "x",
        );
        app.update();

        let late = app
            .world_mut()
            .spawn((RigidBody::Dynamic, Collider::sphere(0.2), ChildOf(hull)))
            .id();
        app.update();

        assert!(app.world().entity(late).contains::<RigidBodyDisabled>());
        assert!(app.world().entity(late).contains::<ColliderDisabled>());
    }

    /// A world-scoped wait pauses the physics clock through the ordinary hold
    /// set, and releases it without touching anyone else's hold.
    #[test]
    fn a_world_wait_holds_physics_and_composes_with_other_holds() {
        let mut app = app();
        app.world_mut()
            .resource_mut::<PhysicsHolds>()
            .set(PhysicsHolds::TERRAIN_READY, true);

        let ticket = app.world_mut().resource_mut::<ReadinessRegistry>().begin(
            Subject::World,
            kinds::SCENE_LOAD,
            "scene.usda",
        );
        app.update();
        assert!(app
            .world()
            .resource::<PhysicsHolds>()
            .holds(PhysicsHolds::READINESS));

        app.world_mut()
            .resource_mut::<ReadinessRegistry>()
            .finish(ticket);
        app.update();
        let holds = app.world().resource::<PhysicsHolds>();
        assert!(!holds.holds(PhysicsHolds::READINESS));
        assert!(
            holds.holds(PhysicsHolds::TERRAIN_READY),
            "releasing readiness must not release the terrain bake"
        );
    }
}
