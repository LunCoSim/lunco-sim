//! The physics readiness gate: "is the world safe to *integrate* right now?"
//!
//! # Why this is not a clock
//!
//! Subsystems routinely need rigid-body integration suspended for a few frames —
//! the DEM heightfield is still baking, a collider ring has not yet paged in under
//! a rover, an obstacle field was just regenerated and its colliders need a frame
//! to re-seat. Step physics during those windows and a `Dynamic` body free-falls
//! through a collider that does not exist yet, tunnels under the heightfield, and
//! is gone.
//!
//! The old code expressed that wait by writing the **user's transport**
//! (`lunco_time::TimeTransport.mode = Paused`). That is a category error with three
//! visible consequences:
//!
//! 1. The sandbox *opened paused*. The holds are up during the first frames of
//!    every scene load, so the user was handed a stopped world and had to press
//!    play to undo an engine implementation detail.
//! 2. It froze **everything**, not just physics — the tick, and with it the epoch,
//!    the ephemeris, the animation sampler, the lighting. A collider that has not
//!    finished baking is no reason for the planets to stop moving.
//! 3. Release needed a "did *we* pause it?" flag (`paused_by_us`) to avoid
//!    un-pausing a pause the user had started themselves — bookkeeping that only
//!    existed because two unrelated concepts shared one bit.
//!
//! So readiness gates **physics**, and nothing else. [`PhysicsHolds`] pauses avian's
//! `Time<Physics>` clock, which zeroes the physics delta and stops the solver while
//! `Time<Virtual>`, the `SimTick`, `WorldTime.epoch_jd`, the celestial chain and the
//! avatar all keep running. The user's transport is never touched, and the world
//! integrates again on its own the moment the last hold clears.
//!
//! Holds are keyed by a `&'static str` reason, so several subsystems can hold
//! concurrently and each releases only its own; physics runs when the set is empty.

use avian3d::prelude::{Physics, PhysicsTime};
use bevy::prelude::*;

pub mod spatial;
pub use spatial::GridSpatialQuery;

/// The set of reasons physics is currently suspended. Empty ⇒ physics integrates.
///
/// This is an **engine** authority. It is not, and must never become, a mirror of
/// the user's play/pause state (`lunco_time::TimeTransport`) — see the module docs.
#[derive(Resource, Debug, Clone, Default)]
pub struct PhysicsHolds {
    reasons: std::collections::BTreeSet<&'static str>,
}

impl PhysicsHolds {
    /// Terrain DEM build / collider-ring warm-up (`lunco-terrain-surface`).
    pub const TERRAIN_READY: &'static str = "terrain-ready";
    /// Obstacle-field regeneration settle window (`lunco-obstacle-field`).
    pub const OBSTACLE_FIELD: &'static str = "obstacle-field";

    /// Is any subsystem holding physics?
    #[inline]
    pub fn is_held(&self) -> bool {
        !self.reasons.is_empty()
    }

    /// Is this specific reason holding?
    #[inline]
    pub fn holds(&self, reason: &'static str) -> bool {
        self.reasons.contains(reason)
    }

    /// Raise or clear one hold. Compare with [`Self::holds`] first so the `ResMut`
    /// is dereferenced only on a real edge (no per-frame change-detection churn).
    pub fn set(&mut self, reason: &'static str, held: bool) {
        let changed = if held {
            self.reasons.insert(reason)
        } else {
            self.reasons.remove(reason)
        };
        if changed {
            debug!(
                "[physics] hold {}: {reason}",
                if held { "raised" } else { "released" }
            );
        }
    }

    /// The reasons currently holding, for diagnostics/UI.
    pub fn reasons(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.reasons.iter().copied()
    }
}

/// Project [`PhysicsHolds`] onto avian's `Time<Physics>`.
///
/// Pausing the physics clock zeroes the physics delta, so the solver does not step
/// — while `Time<Virtual>` (and therefore the tick, epoch, ephemeris and animation)
/// keeps advancing. Runs in `PreUpdate`, ahead of the physics schedule, and is
/// change-driven: it only writes when the desired state differs from the actual, so
/// it is also self-healing if anything pauses the physics clock out of band.
pub fn apply_physics_holds(holds: Res<PhysicsHolds>, mut physics_time: ResMut<Time<Physics>>) {
    let held = holds.is_held();
    if held != physics_time.is_paused() {
        if held {
            physics_time.pause();
        } else {
            physics_time.unpause();
        }
    }
}

/// Installs the physics readiness gate. Add wherever `avian3d`'s `PhysicsPlugins`
/// are added — whoever owns physics owns its gate.
pub struct PhysicsGatePlugin;

impl Plugin for PhysicsGatePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PhysicsHolds>()
            .add_systems(PreUpdate, apply_physics_holds);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_are_reason_keyed_and_release_independently() {
        let mut h = PhysicsHolds::default();
        assert!(!h.is_held());
        h.set(PhysicsHolds::TERRAIN_READY, true);
        h.set(PhysicsHolds::OBSTACLE_FIELD, true);
        assert!(h.is_held());
        // Releasing one leaves the other holding — no subsystem can resume physics
        // on another's behalf.
        h.set(PhysicsHolds::TERRAIN_READY, false);
        assert!(h.is_held());
        assert!(h.holds(PhysicsHolds::OBSTACLE_FIELD));
        h.set(PhysicsHolds::OBSTACLE_FIELD, false);
        assert!(!h.is_held());
    }

    /// The contract: a hold pauses the PHYSICS clock and leaves the virtual clock
    /// (tick → epoch → ephemeris → animation) running. This is what stopped the
    /// sandbox from booting paused, and what keeps the planets moving while a
    /// heightfield bakes.
    #[test]
    fn hold_pauses_physics_clock_only_and_releases_cleanly() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(PhysicsHolds::default());
        world.insert_resource(Time::<Physics>::default());
        world.insert_resource(Time::<Virtual>::default());

        world
            .resource_mut::<PhysicsHolds>()
            .set(PhysicsHolds::TERRAIN_READY, true);
        world.run_system_once(apply_physics_holds).unwrap();
        assert!(world.resource::<Time<Physics>>().is_paused());
        // The virtual clock — and so the tick, the epoch and the celestial chain —
        // is untouched by a physics hold.
        assert!(!world.resource::<Time<Virtual>>().is_paused());

        world
            .resource_mut::<PhysicsHolds>()
            .set(PhysicsHolds::TERRAIN_READY, false);
        world.run_system_once(apply_physics_holds).unwrap();
        assert!(!world.resource::<Time<Physics>>().is_paused());
    }
}
