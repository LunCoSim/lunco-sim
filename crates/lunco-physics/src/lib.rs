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

pub mod escape;
pub mod spatial;
pub use escape::{EscapeDiagnosticPlugin, WorldBounds};
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
    /// A co-simulation model has not finished its first compile
    /// (`lunco-modelica`).
    ///
    /// A cosim model is part of the vehicle, not decoration: a lander's guidance,
    /// a rover's motor controller. Integrating before it exists does not merely
    /// delay it — it simulates a DIFFERENT vehicle, one whose controller outputs
    /// nothing, and the wires into it are dropped as unknown ports because the
    /// stepper that declares them has not been installed. A powered descent
    /// becomes a free fall, and by the time the compile lands (seconds, when MSL
    /// has to load) the vehicle it was meant to fly is already wreckage.
    ///
    /// Held only until each model's first compile SETTLES — success or failure.
    /// A model that fails to compile releases the hold and reports itself; the
    /// world must not be frozen by a broken model.
    pub const COSIM_MODELS: &'static str = "cosim-models";
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
/// Stepping exists because pause/unpause is unusable from inside a script: pausing
/// the world clock stops `FixedUpdate`, so the script that paused it cannot run
/// again to unpause itself. A physics hold keeps the script running, and stepping
/// gives it deterministic control over motion — the same guarantee frame-by-frame
/// capture wants, since the recorder advances virtual time by exactly `1/fps` per
/// captured frame.
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

/// Run condition: **the solver will consume forces applied this tick.**
///
/// Every system that writes into avian's force accumulator (gravity, suspension,
/// wheel drive, thrusters, …) must be gated on this. avian clears the accumulator
/// *inside* the physics step, so a force applied while the step is skipped is never
/// cleared — it is ADDED TO on the next tick, and the next, and discharges in full
/// on the single step that eventually runs.
///
/// MEASURED, and the reason this exists (episode 2, six-wheel rover): shots 1-3 are
/// "frozen" beats, which by design hold *physics* while leaving the *world clock*
/// ticking (see `assets/scripting/prelude/recording.rhai` — pausing the world clock
/// would stop `FixedUpdate` and deadlock the very script that paused it). Across
/// those 28 s ≈ 1800 fixed ticks, `lunco_environment::apply_gravity_to_rigid_bodies`
/// kept adding `m·g` downward with nothing consuming it — ~4 MN accumulated. The
/// hold released at the start of shot 4 and discharged it in ONE step: the rover
/// left the surface at **224.20 m/s downward on that shot's first captured frame**
/// (HUD `elev -2.2`, `speed 224.20 m/s`), which is 3.7 m of travel per 1/60 s step
/// and so tunnels straight through the 1 m ground slab. It was never a fall: it was
/// a launch. Velocity then decayed under the chassis' `linearDamping = 0.5`
/// (169.94 m/s at frame 19, 36.87 m/s at frame 150) — the exponential signature that
/// identified an impulse rather than free fall in the first place.
///
/// Gating on `Time<Physics>` rather than on [`PhysicsHolds`] is deliberate: it is
/// the clock the solver actually integrates, so this is also correct for anything
/// that pauses physics out of band, exactly as [`apply_physics_holds`] is.
///
/// `Time<Virtual>` is NOT a substitute and was the original mistake in
/// `lunco-mobility`: a physics hold does not pause virtual time, so a virtual-clock
/// gate is open for the entire window this needs to close. Both are checked here —
/// a paused world clock must stop force application too, and `Time<Physics>` does
/// not report itself paused merely because virtual time is.
pub fn physics_is_live(physics_time: Res<Time<Physics>>, virtual_time: Res<Time<Virtual>>) -> bool {
    !physics_time.is_paused() && !virtual_time.is_paused() && virtual_time.relative_speed_f64() > 0.0
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

/// Release exactly one queued [`PhysicsStepRequest`] frame through a hold.
///
/// Runs in `FixedPreUpdate`, **inside** the fixed loop — the same clock domain that
/// steps physics (avian integrates in `FixedPostUpdate` off `Time<Fixed>`). A step
/// granted from a render-frame schedule is not equivalent: `Time<Fixed>` only
/// accumulates on frames where virtual time advanced, so a grant landing on a
/// zero-delta frame is consumed without any physics running at all. Offline
/// recording makes that the common case, since it alternates advance and capture
/// frames. Consuming the debt here means one granted step is always exactly one
/// integrated step.
pub fn grant_physics_step(
    holds: Res<PhysicsHolds>,
    mut steps: ResMut<PhysicsStepRequest>,
    mut physics_time: ResMut<Time<Physics>>,
) {
    if !holds.is_held() {
        // Nothing to step past. Drop the debt rather than let it fire later against
        // an unrelated hold (a terrain bake, say).
        if steps.steps > 0 {
            steps.clear();
        }
        return;
    }

    if steps.steps > 0 {
        steps.steps -= 1;
        if physics_time.is_paused() {
            physics_time.unpause();
        }
    } else if !physics_time.is_paused() {
        physics_time.pause();
    }
}

/// Installs the physics readiness gate. Add wherever `avian3d`'s `PhysicsPlugins`
/// are added — whoever owns physics owns its gate.
pub struct PhysicsGatePlugin;

impl Plugin for PhysicsGatePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PhysicsHolds>()
            .init_resource::<PhysicsStepRequest>()
            .add_systems(PreUpdate, apply_physics_holds)
            // Inside the fixed loop, ahead of avian's `FixedPostUpdate` integration,
            // so a granted step coincides with a step that actually runs.
            .add_systems(bevy::prelude::FixedPreUpdate, grant_physics_step)
            // The gate exists because bodies fall through colliders that are not
            // ready yet; the diagnostic reports the ones that got through anyway.
            // Same owner, same plugin — a hold that fails silently is what cost
            // two sessions of eyeballing rendered frames.
            .add_plugins(escape::EscapeDiagnosticPlugin);
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
        world.run_system_once(grant_physics_step).unwrap();
        assert!(
            !world.resource::<Time<Physics>>().is_paused(),
            "the step frame runs"
        );

        // Debt paid: the next fixed step is frozen again without touching the hold.
        world.run_system_once(grant_physics_step).unwrap();
        assert!(world.resource::<Time<Physics>>().is_paused(), "re-freezes");
        assert_eq!(world.resource::<PhysicsStepRequest>().steps, 0);
    }

    /// The step is consumed in the FIXED loop, not on a render frame. Physics
    /// integrates off `Time<Fixed>`, which only accumulates when virtual time
    /// advanced — so a grant made on a zero-delta render frame would be spent
    /// without any physics running. `apply_physics_holds` (render frame) must
    /// therefore leave the debt alone; only `grant_physics_step` may spend it.
    #[test]
    fn render_frame_projection_does_not_spend_the_step_debt() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(PhysicsHolds::default());
        world.insert_resource(PhysicsStepRequest::default());
        world.insert_resource(Time::<Physics>::default());

        world
            .resource_mut::<PhysicsHolds>()
            .set(PhysicsHolds::CINEMATIC, true);
        world.resource_mut::<PhysicsStepRequest>().request(1);

        // Several render frames pass with no fixed step in between.
        for _ in 0..3 {
            world.run_system_once(apply_physics_holds).unwrap();
        }
        assert_eq!(
            world.resource::<PhysicsStepRequest>().steps,
            1,
            "render frames must not burn the step"
        );

        // The fixed step finally runs and spends it.
        world.run_system_once(grant_physics_step).unwrap();
        assert!(!world.resource::<Time<Physics>>().is_paused());
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
        world.run_system_once(grant_physics_step).unwrap();
        assert_eq!(world.resource::<PhysicsStepRequest>().steps, 0);

        world
            .resource_mut::<PhysicsHolds>()
            .set(PhysicsHolds::TERRAIN_READY, true);
        world.run_system_once(apply_physics_holds).unwrap();
        world.run_system_once(grant_physics_step).unwrap();
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
