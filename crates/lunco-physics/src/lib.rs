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
    /// A scripted cutscene / offline recording is choosing when the world moves.
    ///
    /// Held, physics is frozen but `Time<Virtual>` keeps running, so `FixedUpdate` —
    /// and the scenario script driving the shot — stays alive. That is the whole
    /// point: pausing the *world* clock (`lunco_time::TimeTransport`) also stops the
    /// script, so a paused scene can never run the script that would unpause it.
    /// Advance the world from a script with [`PhysicsStepRequest`] instead.
    pub const CINEMATIC: &'static str = "cinematic";

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

/// Frames of physics owed to a caller that is otherwise holding the clock.
///
/// The step half of "hold, then step": with [`PhysicsHolds::CINEMATIC`] raised, a
/// script advances the world deliberately — one frame per step — instead of
/// play/pausing it. Each queued step lets exactly one frame of physics through.
///
/// This exists because pause/unpause is unusable from inside a script: pausing the
/// world clock stops `FixedUpdate`, so the script that paused it never runs again to
/// unpause it (an offline recording then spools frames forever). A physics hold keeps
/// the script running, and stepping gives it deterministic control over motion —
/// which is also exactly what frame-by-frame capture wants, since the recorder
/// already advances virtual time by exactly `1/fps` per captured frame.
///
/// Defaults to zero owed frames, so a build that never touches it behaves as before.
#[derive(Resource, Debug, Clone, Default)]
pub struct PhysicsStepRequest {
    /// Frames of physics still owed. Decremented as each is granted.
    pub steps: u32,
}

impl PhysicsStepRequest {
    /// Queue `n` more frames of physics.
    pub fn request(&mut self, n: u32) {
        self.steps = self.steps.saturating_add(n);
    }

    /// Drop any owed frames (e.g. when the hold is released outright).
    pub fn clear(&mut self) {
        self.steps = 0;
    }
}

/// Project [`PhysicsHolds`] onto avian's `Time<Physics>`.
///
/// Pausing the physics clock zeroes the physics delta, so the solver does not step
/// — while `Time<Virtual>` (and therefore the tick, epoch, ephemeris and animation)
/// keeps advancing. Runs in `PreUpdate`, ahead of the physics schedule, and is
/// change-driven: it only writes when the desired state differs from the actual, so
/// it is also self-healing if anything pauses the physics clock out of band.
pub fn apply_physics_holds(
    holds: Res<PhysicsHolds>,
    mut steps: ResMut<PhysicsStepRequest>,
    mut physics_time: ResMut<Time<Physics>>,
) {
    // A queued step outranks the hold for exactly one frame: physics runs, the debt
    // is paid down, and the next frame re-freezes unless another step is queued.
    // Steps are only meaningful while held — unheld physics is already running, so
    // granting them then would be a no-op that silently burns the request.
    let held = holds.is_held();
    let stepping = held && steps.steps > 0;
    if stepping {
        steps.steps -= 1;
    } else if !held && steps.steps > 0 {
        // Nothing is holding the clock, so there is nothing to step past. Drop the
        // debt rather than let it fire later against an unrelated hold.
        steps.clear();
    }

    let want_paused = held && !stepping;
    if want_paused != physics_time.is_paused() {
        if want_paused {
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
            .init_resource::<PhysicsStepRequest>()
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

    /// A queued step lets exactly ONE frame of physics through a hold, then the
    /// clock re-freezes. This is what lets a cutscene script advance the world
    /// deliberately instead of play/pausing it — pausing the world clock would stop
    /// `FixedUpdate` and the script could never run again to unpause itself.
    #[test]
    fn step_grants_exactly_one_frame_through_a_hold() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(PhysicsHolds::default());
        world.insert_resource(PhysicsStepRequest::default());
        world.insert_resource(Time::<Physics>::default());

        world
            .resource_mut::<PhysicsHolds>()
            .set(PhysicsHolds::CINEMATIC, true);
        world.run_system_once(apply_physics_holds).unwrap();
        assert!(world.resource::<Time<Physics>>().is_paused(), "held ⇒ frozen");

        world.resource_mut::<PhysicsStepRequest>().request(1);
        world.run_system_once(apply_physics_holds).unwrap();
        assert!(
            !world.resource::<Time<Physics>>().is_paused(),
            "the step frame runs"
        );

        // Debt paid: the very next frame is frozen again without touching the hold.
        world.run_system_once(apply_physics_holds).unwrap();
        assert!(world.resource::<Time<Physics>>().is_paused(), "re-freezes");
        assert_eq!(world.resource::<PhysicsStepRequest>().steps, 0);
    }

    /// Steps queued with nothing holding are dropped, not banked — otherwise they
    /// would fire later against an unrelated hold (a terrain bake, say) and leak a
    /// frame of motion into whatever that hold was protecting.
    #[test]
    fn steps_do_not_bank_against_a_future_hold() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(PhysicsHolds::default());
        world.insert_resource(PhysicsStepRequest::default());
        world.insert_resource(Time::<Physics>::default());

        world.resource_mut::<PhysicsStepRequest>().request(3);
        world.run_system_once(apply_physics_holds).unwrap();
        assert_eq!(world.resource::<PhysicsStepRequest>().steps, 0);
        assert!(!world.resource::<Time<Physics>>().is_paused());

        world
            .resource_mut::<PhysicsHolds>()
            .set(PhysicsHolds::TERRAIN_READY, true);
        world.run_system_once(apply_physics_holds).unwrap();
        assert!(
            world.resource::<Time<Physics>>().is_paused(),
            "the later hold is not stepped past by stale debt"
        );
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
        world.insert_resource(PhysicsStepRequest::default());
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
